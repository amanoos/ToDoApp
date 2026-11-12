//! "Forgot your password?": emailing the one-time reset link of [`super::reset`] to the owner
//! of an account.
//!
//! The form answers every request the same way, whether or not the address belongs to an
//! account, so it cannot be used to find out who has one. To keep the *timing* the same too,
//! the lookup, the token and the SMTP conversation all happen in a background task after the
//! response is on its way. Requests are rate limited per network and per address; the
//! per-address limit counts requests for unknown addresses too, so the limit itself tells
//! nothing either.
//!
//! The link is built from `PUBLIC_URL` and never from the request's `Host` header, which a
//! visitor controls: otherwise anyone could have a victim emailed a link to a look-alike site
//! that collects the token. A newer request cancels the earlier link (see
//! [`super::reset::issue`]), so someone who knows an address can make its owner's last link
//! stop working, though not more than a few times an hour and never take the account.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use topcoat::{
    Result,
    context::{Cx, context, try_context},
    mail::{Mail, Mailbox, SmtpTransport, Transport},
};

use super::{
    reset,
    throttle::{Limit, Throttle},
    user::{self, User, normalize_email},
};
use crate::{
    config::EmailConfig,
    db::{Db, db},
};

/// Requests from one network: 10 per 15 minutes.
const REQUESTS_PER_NETWORK: Limit = Limit {
    max: 10,
    window: Duration::from_secs(15 * 60),
};

/// Requests for one address: 3 per hour.
const REQUESTS_PER_ADDRESS: Limit = Limit {
    max: 3,
    window: Duration::from_secs(60 * 60),
};

/// How long to wait for the mail server before giving up on one message.
const SMTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Everything needed to send the reset email. Stored in the router's context when email is
/// configured; its absence means the site offers no "forgot your password?" form.
#[derive(Clone)]
pub struct EmailSender {
    transport: Arc<dyn Transport>,
    from: Mailbox,
    public_url: String,
}

impl EmailSender {
    pub fn new(transport: impl Transport + 'static, from: Mailbox, public_url: String) -> Self {
        Self {
            transport: Arc::new(transport),
            from,
            public_url,
        }
    }

    /// Builds the sender from validated settings. The text of an error never includes the
    /// SMTP URL, which holds the mail account's password.
    pub fn from_config(email: &EmailConfig, public_url: &str) -> std::result::Result<Self, String> {
        let from = email
            .from
            .parse::<Mailbox>()
            .map_err(|_| "MAIL_FROM is not a valid address".to_owned())?;
        let transport = SmtpTransport::from_url(&email.smtp_url)
            .map_err(|_| "SMTP_URL is not a valid SMTP address".to_owned())?
            .timeout(SMTP_TIMEOUT)
            .build();
        Ok(Self::new(transport, from, public_url.to_owned()))
    }

    fn link(&self, token: &str) -> String {
        format!("{}/reset?token={token}", self.public_url)
    }

    /// Sends a short message to `to`, to check that the mail settings work. It holds no link
    /// and no account data.
    pub async fn send_test(&self, to: &str) -> Result<()> {
        let to = Mailbox::new(to).map_err(std::io::Error::other)?;
        let mail = Mail::builder()
            .from(self.from.clone())
            .to([to])
            .subject("Test message")
            .text(format!(
                "The mail settings of {} work: this message was accepted by the mail server.\n",
                self.public_url
            ))
            .build()
            .map_err(std::io::Error::other)?;
        self.send(mail).await
    }

    /// Sends a mail. The SMTP transport ignores the context, so a fresh one serves a
    /// background task that outlives its request.
    async fn send(&self, mail: Mail) -> Result<()> {
        self.transport.send(&Cx::new(), mail).await?;
        Ok(())
    }
}

/// The message asking someone to choose a new password.
fn reset_mail(sender: &EmailSender, to: &str, token: &str) -> Result<Mail> {
    let minutes = reset::RESET_LIFETIME.as_secs() / 60;
    let body = format!(
        "Someone asked to reset the password of your account.\n\
         \n\
         To choose a new password, open this link (it works once, for {minutes} minutes):\n\
         \n\
         {link}\n\
         \n\
         Choosing a new password signs you out on every device.\n\
         \n\
         If you did not ask for this, ignore this message: your password has not changed and \
         nobody has been given access.\n",
        link = sender.link(token),
    );
    let to = Mailbox::new(to).map_err(std::io::Error::other)?;
    Mail::builder()
        .from(sender.from.clone())
        .to([to])
        .subject("Reset your password")
        .text(body)
        .build()
        .map_err(|error| std::io::Error::other(error).into())
}

/// What to do with one request, decided by the rate limits alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Look the address up and send mail if there is an account.
    Send,
    /// This address was asked about too often, or is not an address. The response is the
    /// same as for [`Send`](Self::Send); nothing happens.
    Ignore,
    /// This network is asking too often, whatever it asks about. Safe to say so out loud,
    /// since it reveals nothing about any account.
    TooManyFromNetwork,
}

/// Counts a request and decides what happens to it. Every request counts against the
/// network; every request for a well-formed address counts against that address.
pub fn admit(throttle: &Throttle, network: &str, email: Option<&str>, now: Instant) -> Admission {
    let network_key = format!("forgot:network:{network}");
    if throttle.is_blocked(&network_key, REQUESTS_PER_NETWORK, now) {
        return Admission::TooManyFromNetwork;
    }
    throttle.record(&network_key, REQUESTS_PER_NETWORK, now);

    let Some(email) = email else {
        return Admission::Ignore;
    };
    let address_key = format!("forgot:address:{email}");
    if throttle.is_blocked(&address_key, REQUESTS_PER_ADDRESS, now) {
        return Admission::Ignore;
    }
    throttle.record(&address_key, REQUESTS_PER_ADDRESS, now);
    Admission::Send
}

/// Whether this site can email reset links.
pub fn enabled(cx: &Cx) -> bool {
    try_context::<EmailSender>(cx).is_some()
}

/// Handles one submission of the form: applies the rate limits and, if the request is
/// admitted, hands the rest to a background task. Returns quickly and identically whether or
/// not the address has an account.
pub fn request(cx: &Cx, raw_email: &str) -> Admission {
    let Some(sender) = try_context::<EmailSender>(cx).cloned() else {
        return Admission::Ignore;
    };
    let email = normalize_email(raw_email);
    let admission = admit(
        context::<Throttle>(cx),
        &user::client_network_key(cx),
        email.as_deref(),
        Instant::now(),
    );
    if admission == Admission::Send
        && let Some(email) = email
    {
        let handle = db(cx);
        tokio::spawn(async move {
            if let Err(error) = deliver(&handle, &sender, &email, reset::now()).await {
                // Neither the address nor the link is logged.
                eprintln!("could not email a password reset link: {error}");
            }
        });
    }
    admission
}

/// Emails a fresh reset link to the account with this (already normalized) address, if there
/// is one. Returns whether a message was sent.
pub async fn deliver(db: &Db, sender: &EmailSender, email: &str, now: i64) -> Result<bool> {
    let Some(user) = User::find_by_email(db, email).await? else {
        return Ok(false);
    };
    let token = reset::issue(db, user.id, now).await?;
    sender
        .send(reset_mail(sender, &user.email, &token)?)
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use topcoat::mail::MemoryTransport;

    use super::*;
    use crate::db::test_cx;
    use crate::features::{reset::ResetError, user::create_account};

    const OLD: &str = "the old password 123";
    const NEW: &str = "a brand new password 456";
    const NOW: i64 = 1_000_000;
    const SITE: &str = "https://todo.example.com";

    fn sender() -> (EmailSender, MemoryTransport) {
        let transport = MemoryTransport::new();
        let from = "Todo <noreply@example.com>".parse().unwrap();
        (
            EmailSender::new(transport.clone(), from, SITE.to_owned()),
            transport,
        )
    }

    async fn account(cx: &Cx, email: &str) -> User {
        create_account(&db(cx), email, OLD)
            .await
            .unwrap()
            .expect("account created")
    }

    /// The token in the one link a message contains.
    fn token_in(mail: &Mail) -> String {
        let text = mail.text().expect("a plain text body");
        let prefix = format!("{SITE}/reset?token=");
        let start = text.find(&prefix).expect("a reset link") + prefix.len();
        text[start..start + 64].to_owned()
    }

    #[tokio::test]
    async fn an_account_is_emailed_a_link_that_resets_its_password() {
        let cx = test_cx().await;
        let user = account(&cx, "ada@example.com").await;
        let (sender, transport) = sender();

        assert!(
            deliver(&db(&cx), &sender, "ada@example.com", NOW)
                .await
                .unwrap()
        );

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        let mail = &sent[0];
        assert_eq!(mail.subject(), "Reset your password");
        assert_eq!(mail.to().len(), 1);
        assert_eq!(mail.to()[0].address(), "ada@example.com");
        assert!(mail.cc().is_empty() && mail.bcc().is_empty());
        assert_eq!(mail.from().address(), "noreply@example.com");

        // The link in the message really resets the password, exactly once.
        let token = token_in(mail);
        reset::redeem(&db(&cx), &token, NEW, NOW + 60)
            .await
            .unwrap()
            .expect("the emailed link works");
        assert!(
            user::verify_current_password(&cx, user.id, NEW)
                .await
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            reset::redeem(&db(&cx), &token, NEW, NOW + 61)
                .await
                .unwrap(),
            Err(ResetError::InvalidLink)
        );
    }

    #[tokio::test]
    async fn an_unknown_address_gets_no_email_and_leaves_no_trace() {
        let cx = test_cx().await;
        account(&cx, "ada@example.com").await;
        let (sender, transport) = sender();

        assert!(
            !deliver(&db(&cx), &sender, "nobody@example.com", NOW)
                .await
                .unwrap()
        );
        assert!(transport.sent().is_empty());
        assert_eq!(reset::PasswordReset::all(&db(&cx)).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_new_email_cancels_the_link_in_the_previous_one() {
        let cx = test_cx().await;
        account(&cx, "ada@example.com").await;
        let (sender, transport) = sender();

        deliver(&db(&cx), &sender, "ada@example.com", NOW)
            .await
            .unwrap();
        deliver(&db(&cx), &sender, "ada@example.com", NOW + 10)
            .await
            .unwrap();

        let sent = transport.sent();
        let (first, second) = (token_in(&sent[0]), token_in(&sent[1]));
        assert_ne!(first, second);
        assert!(
            reset::find_valid(&db(&cx), &first, NOW + 20)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            reset::find_valid(&db(&cx), &second, NOW + 20)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn the_link_comes_from_the_configured_address_and_the_message_says_how_long_it_lasts() {
        let cx = test_cx().await;
        account(&cx, "ada@example.com").await;
        let transport = MemoryTransport::new();
        let sender = EmailSender::new(
            transport.clone(),
            "noreply@example.com".parse().unwrap(),
            "https://mine.example.org".to_owned(),
        );
        deliver(&db(&cx), &sender, "ada@example.com", NOW)
            .await
            .unwrap();
        let text = transport.sent()[0].text().unwrap().to_owned();
        assert!(
            text.contains("https://mine.example.org/reset?token="),
            "{text}"
        );
        assert!(!text.contains(SITE));
        assert!(text.contains("60 minutes"), "{text}");
    }

    #[test]
    fn requests_for_one_address_are_limited_and_the_limit_looks_the_same_for_every_address() {
        let throttle = Throttle::new();
        let now = Instant::now();
        // Known or unknown makes no difference to the limiter: it only sees the text.
        for address in ["ada@example.com", "ghost@example.com"] {
            for _ in 0..3 {
                assert_eq!(
                    admit(&throttle, "net-a", Some(address), now),
                    Admission::Send
                );
            }
            assert_eq!(
                admit(&throttle, "net-a", Some(address), now),
                Admission::Ignore
            );
        }
        // Another address is unaffected, and so is the same address an hour later.
        assert_eq!(
            admit(&throttle, "net-b", Some("bob@example.com"), now),
            Admission::Send
        );
        assert_eq!(
            admit(
                &throttle,
                "net-b",
                Some("ada@example.com"),
                now + Duration::from_secs(3601)
            ),
            Admission::Send
        );
    }

    #[test]
    fn one_network_cannot_ask_forever_by_varying_the_address() {
        let throttle = Throttle::new();
        let now = Instant::now();
        for i in 0..10 {
            let email = format!("user{i}@example.com");
            assert_eq!(
                admit(&throttle, "net-a", Some(&email), now),
                Admission::Send
            );
        }
        assert_eq!(
            admit(&throttle, "net-a", Some("one-more@example.com"), now),
            Admission::TooManyFromNetwork
        );
        // Not-an-address requests count against the network too.
        assert_eq!(
            admit(&throttle, "net-a", None, now),
            Admission::TooManyFromNetwork
        );
        // Another network, and the same one later, are fine.
        assert_eq!(
            admit(&throttle, "net-b", Some("x@example.com"), now),
            Admission::Send
        );
        assert_eq!(
            admit(
                &throttle,
                "net-a",
                Some("x@example.com"),
                now + Duration::from_secs(15 * 60 + 1)
            ),
            Admission::Send
        );
    }

    #[test]
    fn something_that_is_not_an_address_is_ignored() {
        let throttle = Throttle::new();
        assert_eq!(
            admit(&throttle, "net-a", None, Instant::now()),
            Admission::Ignore
        );
    }

    #[tokio::test]
    async fn a_request_sends_in_the_background_and_answers_alike_for_known_and_unknown_addresses() {
        let (sender, transport) = sender();
        let cx = test_cx().await.with(sender);
        account(&cx, "ada@example.com").await;

        assert!(enabled(&cx));
        let known = request(&cx, "  Ada@Example.com ");
        let unknown = request(&cx, "ghost@example.com");
        assert_eq!(known, unknown);
        assert_eq!(known, Admission::Send);

        // The message arrives from the background task, for the account only.
        for _ in 0..200 {
            if !transport.sent().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to()[0].address(), "ada@example.com");
    }

    #[tokio::test]
    async fn the_test_message_carries_no_link() {
        let (sender, transport) = sender();
        sender.send_test("ops@example.com").await.unwrap();
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to()[0].address(), "ops@example.com");
        assert!(!sent[0].text().unwrap().contains("token"));
        assert!(sender.send_test("not an address").await.is_err());
    }

    #[tokio::test]
    async fn without_email_configured_nothing_is_offered_or_sent() {
        let cx = test_cx().await;
        account(&cx, "ada@example.com").await;
        assert!(!enabled(&cx));
        assert_eq!(request(&cx, "ada@example.com"), Admission::Ignore);
    }
}
