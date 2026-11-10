//! Password reset by one-time link.
//!
//! The site cannot send email, so the *operator* makes a link (`todo-app reset-link <email>`)
//! and gives it to the person. The link carries a random token. Only a SHA-256 hash of the
//! token is stored, so a copy of the database yields no usable links. A token works once,
//! for one hour, and a person has at most one live token: making a new one cancels the old.
//! Using a token sets the new password and signs the account out everywhere.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use topcoat::{
    Result,
    context::{Cx, context},
};

use super::{
    throttle::{Limit, Throttle},
    user::{self, SessionRecord, User},
};
use crate::db::db;

/// How long a reset link works.
pub const RESET_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Bad links tried from one network: 20 per 15 minutes. A token cannot be guessed (it has
/// 256 bits), so this is only about not letting a client hammer the endpoint.
const BAD_LINKS_PER_NETWORK: Limit = Limit {
    max: 20,
    window: Duration::from_secs(15 * 60),
};

/// The current time in whole seconds since the Unix epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// A live reset token: who it is for and when it stops working. The token itself is never
/// stored, only its hash.
#[derive(Debug, toasty::Model)]
pub struct PasswordReset {
    #[key]
    #[auto]
    pub id: u64,

    #[index]
    pub user_id: u64,

    /// SHA-256 of the token.
    #[unique]
    pub token_hash: Vec<u8>,

    /// Seconds since the Unix epoch.
    pub expires_at: i64,
}

/// Why a reset was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetError {
    /// The link is wrong, expired, already used, or replaced by a newer one.
    InvalidLink,
    /// The new password is too short or too long.
    WeakPassword,
    /// The new password and its confirmation differ.
    Mismatch,
    /// Too many bad links tried from this network recently.
    Throttled,
}

impl ResetError {
    /// The message shown on the form.
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidLink => {
                "This reset link is not valid. It may have expired or already been used."
            }
            Self::WeakPassword => "Use a new password of 12 to 128 characters.",
            Self::Mismatch => "The new password and its confirmation do not match.",
            Self::Throttled => "Too many attempts. Please wait a few minutes and try again.",
        }
    }

    /// A short code carried in the redirect back to the form.
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidLink => "invalid",
            Self::WeakPassword => "weak",
            Self::Mismatch => "mismatch",
            Self::Throttled => "throttled",
        }
    }

    /// The inverse of [`ResetError::code`].
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::InvalidLink,
            Self::WeakPassword,
            Self::Mismatch,
            Self::Throttled,
        ]
        .into_iter()
        .find(|error| error.code() == code)
    }
}

/// A fresh random token: 32 random bytes as 64 lower-case hex characters (256 bits, far
/// beyond guessing).
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Accepts a token as typed or pasted: surrounding spaces and upper case are forgiven, and
/// anything that is not 64 hex characters is refused without touching the database.
pub(crate) fn clean_token(raw: &str) -> Option<String> {
    let token = raw.trim();
    (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

/// The hash that is stored in place of the token.
fn digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Makes a reset token for the user, valid from `now` (Unix seconds) for
/// [`RESET_LIFETIME`], and returns it. This is the only time the token exists in the clear.
/// Any earlier token for the same user stops working.
pub async fn issue(db: &mut toasty::Db, user_id: u64, now: i64) -> Result<String> {
    let token = new_token();
    let expires_at = now.saturating_add(i64::try_from(RESET_LIFETIME.as_secs()).unwrap_or(3600));

    let mut tx = db.transaction().await?;
    PasswordReset::filter_by_user_id(user_id)
        .delete()
        .exec(&mut tx)
        .await?;
    toasty::create!(PasswordReset {
        user_id,
        token_hash: digest(&token),
        expires_at,
    })
    .exec(&mut tx)
    .await?;
    tx.commit().await?;
    Ok(token)
}

/// The live token record this text stands for, if it is well formed, known and has not
/// expired by `now` (Unix seconds).
pub async fn find_valid(db: &mut toasty::Db, raw: &str, now: i64) -> Result<Option<PasswordReset>> {
    let Some(token) = clean_token(raw) else {
        return Ok(None);
    };
    let found = PasswordReset::filter_by_token_hash(digest(&token))
        .first()
        .exec(db)
        .await?;
    Ok(found.filter(|reset| reset.expires_at > now))
}

/// Uses a reset token to set a new password.
///
/// The token is checked before anything expensive happens. Then, in one transaction, the
/// password is replaced, the person's reset tokens are all deleted (so this one cannot be
/// used again) and every session of the account is deleted (so anyone who had access with
/// the old password is signed out). Nobody is logged in by this: the person signs in with the
/// new password. A password that is refused leaves the token intact, so they can try again.
pub async fn redeem(
    db: &mut toasty::Db,
    raw: &str,
    new_password: &str,
    now: i64,
) -> Result<std::result::Result<(), ResetError>> {
    let Some(token) = clean_token(raw) else {
        return Ok(Err(ResetError::InvalidLink));
    };
    if find_valid(db, &token, now).await?.is_none() {
        return Ok(Err(ResetError::InvalidLink));
    }
    if !user::password_is_acceptable(new_password) {
        return Ok(Err(ResetError::WeakPassword));
    }
    let new_hash = user::hash_new_password(new_password).await?;

    let mut tx = db.transaction().await?;
    // Checked again inside the transaction, so two people using one link at once cannot both
    // succeed. Returning early drops the transaction, which rolls it back.
    let Some(reset) = PasswordReset::filter_by_token_hash(digest(&token))
        .first()
        .exec(&mut tx)
        .await?
        .filter(|reset| reset.expires_at > now)
    else {
        return Ok(Err(ResetError::InvalidLink));
    };
    let Some(mut account) = User::filter_by_id(reset.user_id)
        .first()
        .exec(&mut tx)
        .await?
    else {
        return Ok(Err(ResetError::InvalidLink));
    };

    account
        .update()
        .password_hash(new_hash.as_str())
        .exec(&mut tx)
        .await?;
    PasswordReset::filter_by_user_id(reset.user_id)
        .delete()
        .exec(&mut tx)
        .await?;
    SessionRecord::filter_by_user_id(reset.user_id)
        .delete()
        .exec(&mut tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// [`redeem`] for a web request: the same, plus the checks that belong to a request.
///
/// Bad links are counted per network and, past the limit, refused outright (before any
/// lookup). The link is checked *first*, so a wrong link always reads as wrong whatever else
/// was typed, and a mismatch between the two passwords is only ever reported to someone
/// holding a valid link, and then leaves the link usable.
pub async fn redeem_for_request(
    cx: &Cx,
    raw: &str,
    new_password: &str,
    confirm: &str,
    now: i64,
) -> Result<std::result::Result<(), ResetError>> {
    let throttle = context::<Throttle>(cx);
    let key = format!("reset:ip:{}", user::client_network_key(cx));
    if throttle.is_blocked(&key, BAD_LINKS_PER_NETWORK, Instant::now()) {
        return Ok(Err(ResetError::Throttled));
    }

    let mut handle = db(cx);
    if find_valid(&mut handle, raw, now).await?.is_none() {
        throttle.record(&key, BAD_LINKS_PER_NETWORK, Instant::now());
        return Ok(Err(ResetError::InvalidLink));
    }
    if new_password != confirm {
        return Ok(Err(ResetError::Mismatch));
    }

    let outcome = redeem(&mut handle, raw, new_password, now).await?;
    if outcome == Err(ResetError::InvalidLink) {
        throttle.record(&key, BAD_LINKS_PER_NETWORK, Instant::now());
    }
    Ok(outcome)
}

/// Deletes every reset token that has expired by `now` (Unix seconds). Expired tokens
/// already do nothing; this only keeps the table from growing.
pub async fn purge_expired(db: &mut toasty::Db, now: i64) -> Result<()> {
    PasswordReset::filter(PasswordReset::fields().expires_at().le(now))
        .delete()
        .exec(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::db;
    use crate::features::user::{create_account, verify_current_password};
    use topcoat::context::Cx;

    const OLD: &str = "the old password 123";
    const NEW: &str = "a brand new password 456";
    const NOW: i64 = 1_000_000;

    async fn account(cx: &Cx, email: &str) -> User {
        create_account(&mut db(cx), email, OLD)
            .await
            .unwrap()
            .expect("account created")
    }

    async fn live_tokens(cx: &Cx, user_id: u64) -> usize {
        PasswordReset::filter_by_user_id(user_id)
            .exec(&mut db(cx))
            .await
            .unwrap()
            .len()
    }

    async fn sessions(cx: &Cx, user_id: u64) -> usize {
        SessionRecord::filter_by_user_id(user_id)
            .exec(&mut db(cx))
            .await
            .unwrap()
            .len()
    }

    async fn add_session(cx: &Cx, user_id: u64, marker: u8) {
        toasty::create!(SessionRecord {
            token_hash: vec![marker; 32],
            user_id,
            expires_at: 9_999_999_999i64,
        })
        .exec(&mut db(cx))
        .await
        .unwrap();
    }

    /// Whether `password` is now the account's password.
    async fn password_is(cx: &Cx, user_id: u64, password: &str) -> bool {
        verify_current_password(cx, user_id, password)
            .await
            .unwrap()
            .is_ok()
    }

    async fn redeem_at(
        cx: &Cx,
        token: &str,
        password: &str,
        now: i64,
    ) -> std::result::Result<(), ResetError> {
        redeem(&mut db(cx), token, password, now)
            .await
            .expect("no internal error")
    }

    fn hex_bytes(token: &str) -> Vec<u8> {
        (0..token.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&token[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn tokens_are_long_random_hex_and_never_repeat() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_eq!(clean_token(&a), Some(a));
    }

    #[test]
    fn pasted_tokens_are_forgiven_but_malformed_ones_are_refused() {
        let token = new_token();
        assert_eq!(
            clean_token(&format!("  {}\n", token.to_uppercase())),
            Some(token.clone())
        );
        for bad in [
            "",
            "abc",
            &token[..63],
            &format!("{token}0"),
            &format!("{}g", &token[..63]),
            &"z".repeat(64),
            "../../etc/passwd",
            "' OR 1=1 --",
        ] {
            assert_eq!(clean_token(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_stored_digest_is_a_sha256_that_is_not_the_token() {
        let hash = digest("00");
        assert_eq!(hash.len(), 32);
        assert_ne!(hash, b"00");
        assert_eq!(
            hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "f1534392279bddbf9d43dde8701cb5be14b82f76ec6607bf8d6ad557f60f304e",
            "SHA-256 of the two characters 00"
        );
    }

    #[tokio::test]
    async fn a_new_token_works_once_and_sets_the_password() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        assert!(
            find_valid(&mut db(&cx), &token, NOW)
                .await
                .unwrap()
                .is_some()
        );

        assert_eq!(redeem_at(&cx, &token, NEW, NOW).await, Ok(()));
        assert!(
            password_is(&cx, alice.id, NEW).await,
            "the new password works"
        );
        assert!(
            !password_is(&cx, alice.id, OLD).await,
            "the old one does not"
        );

        // The same link cannot be used a second time.
        assert_eq!(
            redeem_at(&cx, &token, "yet another password 789", NOW).await,
            Err(ResetError::InvalidLink)
        );
        assert!(password_is(&cx, alice.id, NEW).await, "and changed nothing");
        assert_eq!(live_tokens(&cx, alice.id).await, 0);
    }

    #[tokio::test]
    async fn the_raw_token_is_never_stored() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();

        let stored = PasswordReset::all().exec(&mut db(&cx)).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_ne!(stored[0].token_hash, token.as_bytes());
        assert_ne!(stored[0].token_hash, hex_bytes(&token));
        assert_eq!(stored[0].token_hash, digest(&token));
    }

    #[tokio::test]
    async fn a_link_stops_working_exactly_when_it_expires() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        let lifetime = i64::try_from(RESET_LIFETIME.as_secs()).unwrap();

        assert!(
            find_valid(&mut db(&cx), &token, NOW + lifetime - 1)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_valid(&mut db(&cx), &token, NOW + lifetime)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            redeem_at(&cx, &token, NEW, NOW + lifetime).await,
            Err(ResetError::InvalidLink)
        );
        assert!(
            password_is(&cx, alice.id, OLD).await,
            "an expired link changes nothing"
        );
    }

    #[tokio::test]
    async fn making_a_new_link_cancels_the_old_one() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let first = issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        let second = issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        assert_ne!(first, second);
        assert_eq!(
            live_tokens(&cx, alice.id).await,
            1,
            "one live token per person"
        );

        assert_eq!(
            redeem_at(&cx, &first, NEW, NOW).await,
            Err(ResetError::InvalidLink)
        );
        assert_eq!(redeem_at(&cx, &second, NEW, NOW).await, Ok(()));
    }

    #[tokio::test]
    async fn using_a_link_signs_the_account_out_everywhere_and_only_that_account() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let bob = account(&cx, "bob@example.com").await;
        add_session(&cx, alice.id, 1).await;
        add_session(&cx, alice.id, 2).await;
        add_session(&cx, bob.id, 3).await;
        let bobs_token = issue(&mut db(&cx), bob.id, NOW).await.unwrap();
        let token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();

        assert_eq!(redeem_at(&cx, &token, NEW, NOW).await, Ok(()));
        assert_eq!(
            sessions(&cx, alice.id).await,
            0,
            "every session of hers is revoked"
        );
        assert_eq!(sessions(&cx, bob.id).await, 1, "Bob's session is untouched");
        assert_eq!(live_tokens(&cx, bob.id).await, 1, "and so is his link");
        assert!(password_is(&cx, bob.id, OLD).await, "and his password");
        assert!(
            find_valid(&mut db(&cx), &bobs_token, NOW)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_refused_password_leaves_the_link_usable_for_another_try() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        add_session(&cx, alice.id, 1).await;
        let token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();

        for weak in ["short", "", &"x".repeat(129)] {
            assert_eq!(
                redeem_at(&cx, &token, weak, NOW).await,
                Err(ResetError::WeakPassword)
            );
        }
        assert!(password_is(&cx, alice.id, OLD).await, "nothing changed");
        assert_eq!(sessions(&cx, alice.id).await, 1, "nobody was signed out");
        assert_eq!(
            redeem_at(&cx, &token, NEW, NOW).await,
            Ok(()),
            "the link still works"
        );
    }

    #[tokio::test]
    async fn a_wrong_or_malformed_token_gets_the_same_answer_as_an_expired_one() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let _token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        for bad in ["", "nonsense", &"0".repeat(64), &"f".repeat(64)] {
            assert_eq!(
                redeem_at(&cx, bad, NEW, NOW).await,
                Err(ResetError::InvalidLink),
                "{bad:?}"
            );
        }
        assert!(password_is(&cx, alice.id, OLD).await);
    }

    #[tokio::test]
    async fn a_token_for_an_account_that_no_longer_exists_does_nothing() {
        let cx = crate::db::test_cx().await;
        let ghost = issue(&mut db(&cx), 424_242, NOW).await.unwrap();
        assert_eq!(
            redeem_at(&cx, &ghost, NEW, NOW).await,
            Err(ResetError::InvalidLink)
        );
    }

    #[tokio::test]
    async fn purging_removes_only_expired_tokens() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let bob = account(&cx, "bob@example.com").await;
        issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        issue(&mut db(&cx), bob.id, NOW + 10_000).await.unwrap();

        let lifetime = i64::try_from(RESET_LIFETIME.as_secs()).unwrap();
        purge_expired(&mut db(&cx), NOW + lifetime).await.unwrap();
        assert_eq!(live_tokens(&cx, alice.id).await, 0, "hers had expired");
        assert_eq!(live_tokens(&cx, bob.id).await, 1, "his is still live");
    }

    async fn via_request(
        cx: &Cx,
        token: &str,
        new: &str,
        confirm: &str,
    ) -> std::result::Result<(), ResetError> {
        redeem_for_request(cx, token, new, confirm, NOW)
            .await
            .expect("no internal error")
    }

    #[tokio::test]
    async fn a_wrong_link_reads_as_wrong_whatever_else_was_typed() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        // Mismatching passwords with a bad link: told the link is bad, nothing about the passwords.
        assert_eq!(
            via_request(&cx, &"a".repeat(64), NEW, "something else").await,
            Err(ResetError::InvalidLink)
        );
        assert_eq!(
            via_request(&cx, "nonsense", "short", "short").await,
            Err(ResetError::InvalidLink)
        );
    }

    #[tokio::test]
    async fn a_mismatch_is_reported_to_a_valid_link_and_leaves_it_usable() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();
        for _ in 0..25 {
            assert_eq!(
                via_request(&cx, &token, NEW, "different").await,
                Err(ResetError::Mismatch)
            );
        }
        // Mistakes by someone with a valid link are not counted against the network.
        assert_eq!(via_request(&cx, &token, NEW, NEW).await, Ok(()));
        assert!(password_is(&cx, alice.id, NEW).await);
    }

    #[tokio::test]
    async fn too_many_bad_links_from_one_network_are_refused_even_a_good_one() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let token = issue(&mut db(&cx), alice.id, NOW).await.unwrap();

        for attempt in 1..=BAD_LINKS_PER_NETWORK.max {
            assert_eq!(
                via_request(&cx, &"0".repeat(64), NEW, NEW).await,
                Err(ResetError::InvalidLink),
                "attempt {attempt}"
            );
        }
        assert_eq!(
            via_request(&cx, &token, NEW, NEW).await,
            Err(ResetError::Throttled),
            "refused before the link is even looked at"
        );
        assert!(password_is(&cx, alice.id, OLD).await, "and nothing changed");
    }

    #[test]
    fn reset_error_codes_round_trip() {
        for error in [
            ResetError::InvalidLink,
            ResetError::WeakPassword,
            ResetError::Mismatch,
            ResetError::Throttled,
        ] {
            assert_eq!(ResetError::from_code(error.code()), Some(error));
            assert!(!error.message().is_empty());
        }
        assert_eq!(ResetError::from_code("nope"), None);
    }
}
