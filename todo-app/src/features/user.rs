//! Accounts and sessions.
//!
//! Topcoat issues and carries the session token (a cookie); this module stores who the
//! token belongs to. Only the SHA-256 hash of a token is stored, never the token itself.

use std::{
    net::IpAddr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use topcoat::{
    Result,
    context::{Cx, context},
    router::request::client_ip,
    session::{self, Session, TokenHash},
};

use super::throttle::{Limit, Throttle};
use crate::config::Registration;
use crate::db::db;

/// How long a session lasts without activity. Active sessions are extended; see
/// [`refresh_session`].
pub const SESSION_LIFETIME: Duration = Duration::from_secs(60 * 60 * 24 * 14);

/// Wrong-password limit for one email from one network: 5 failures per 15 minutes. Keyed
/// by network *and* email, so a stranger can't lock the real owner out of their account
/// from a different network.
const LOGIN_PER_ACCOUNT: Limit = Limit {
    max: 5,
    window: Duration::from_secs(15 * 60),
};
/// Failure limit for one network across all emails: 20 per 15 minutes, which stops
/// one client from trying many accounts.
const LOGIN_PER_IP: Limit = Limit {
    max: 20,
    window: Duration::from_secs(15 * 60),
};
/// Registration attempts from one network: 10 per hour.
const REGISTER_PER_IP: Limit = Limit {
    max: 10,
    window: Duration::from_secs(60 * 60),
};
/// Wrong "current password" answers for one account: 5 per 15 minutes. Someone holding a
/// stolen session can reach the password form, so guessing the current password there must
/// be as hard as guessing it at login.
const PASSWORD_PER_ACCOUNT: Limit = Limit {
    max: 5,
    window: Duration::from_secs(15 * 60),
};
/// The same failures counted per network: 20 per 15 minutes.
const PASSWORD_PER_IP: Limit = Limit {
    max: 20,
    window: Duration::from_secs(15 * 60),
};

/// Shortest password accepted at registration.
pub const MIN_PASSWORD_LEN: usize = 12;
/// Longest password accepted, which bounds the work a single request can ask of Argon2.
pub const MAX_PASSWORD_LEN: usize = 128;
/// Longest email address, as in RFC 5321.
pub const MAX_EMAIL_LEN: usize = 320;

/// A person with an account.
#[derive(Debug, toasty::Model)]
pub struct User {
    #[key]
    #[auto]
    pub id: u64,

    /// Lower-cased and trimmed.
    #[unique]
    pub email: String,

    /// An Argon2id hash in PHC string format, which includes its own salt.
    pub password_hash: String,
}

/// A logged-in session: which user a token hash belongs to, and until when.
#[derive(Debug, toasty::Model)]
pub struct SessionRecord {
    #[key]
    #[auto]
    pub id: u64,

    #[unique]
    pub token_hash: Vec<u8>,

    #[index]
    pub user_id: u64,

    /// Seconds since the Unix epoch.
    pub expires_at: i64,
}

/// Why registration or login was refused. Shown to the person, so keep it generic where
/// it could reveal whether an account exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    InvalidEmail,
    WeakPassword,
    EmailTaken,
    BadCredentials,
    /// Too many recent attempts from this client.
    Throttled,
    /// New accounts cannot be created through the sign-up page on this site.
    Closed,
}

impl AuthError {
    /// The message shown on the form.
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidEmail => "Enter a valid email address.",
            Self::WeakPassword => "Use a password of 12 to 128 characters.",
            Self::EmailTaken => "That email address can't be used. Try signing in instead.",
            Self::BadCredentials => "Wrong email or password.",
            Self::Throttled => "Too many attempts. Please wait a few minutes and try again.",
            Self::Closed => "Sign-ups are closed. Ask the owner of this site for an account.",
        }
    }

    /// A short code carried in the redirect back to the form.
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidEmail => "email",
            Self::WeakPassword => "password",
            Self::EmailTaken => "taken",
            Self::BadCredentials => "credentials",
            Self::Throttled => "throttled",
            Self::Closed => "closed",
        }
    }

    /// The inverse of [`AuthError::code`].
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::InvalidEmail,
            Self::WeakPassword,
            Self::EmailTaken,
            Self::BadCredentials,
            Self::Throttled,
            Self::Closed,
        ]
        .into_iter()
        .find(|error| error.code() == code)
    }
}

/// Trims and lower-cases an email address, or returns `None` if it is not plausible.
///
/// This is a sanity check, not full RFC validation: one `@`, something on both sides,
/// a dot in the domain, no whitespace.
pub fn normalize_email(input: &str) -> Option<String> {
    let email = input.trim().to_lowercase();
    if email.len() > MAX_EMAIL_LEN || email.chars().any(char::is_whitespace) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    let valid = !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.');
    valid.then_some(email)
}

/// Whether a password meets the length rules. Counts characters, not bytes.
pub fn password_is_acceptable(password: &str) -> bool {
    let len = password.chars().count();
    (MIN_PASSWORD_LEN..=MAX_PASSWORD_LEN).contains(&len)
}

/// Hashes a password with Argon2id and a fresh random salt.
fn hash_password(password: &str) -> std::result::Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

/// The rate-limit key for the network the current request came from (see [`ip_key`]).
pub(crate) fn client_network_key(cx: &Cx) -> String {
    ip_key(client_ip(cx))
}

/// Hashes a new password with Argon2id, off the async executor threads (Argon2 is CPU-heavy
/// by design). The one place other modules go through to turn a password into a stored hash.
pub async fn hash_new_password(password: &str) -> Result<String> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|error| other_error(format!("password hashing task failed: {error}")))?
        .map_err(|error| other_error(format!("hashing the password failed: {error}")))
}

/// Checks a password against a stored hash.
fn verify_password(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
        .unwrap_or(false)
}

/// A valid Argon2 hash that matches no real password. Logging in as an unknown email
/// verifies against it, so the response takes as long as for a known email.
fn dummy_hash() -> &'static str {
    use std::sync::OnceLock;
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hash_password("not-a-real-password").expect("hashing a constant works"))
}

fn unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

fn other_error(message: String) -> topcoat::Error {
    std::io::Error::other(message).into()
}

/// Whether the sign-up page may create an account right now, under the site's
/// [`Registration`] setting.
pub async fn registration_open(cx: &Cx) -> Result<bool> {
    match *context::<Registration>(cx) {
        Registration::Open => Ok(true),
        Registration::Closed => Ok(false),
        // Open only until there is a first account.
        Registration::FirstUser => Ok(User::all().first().exec(&mut db(cx)).await?.is_none()),
    }
}

/// In `first-user` mode, settles a race between simultaneous sign-ups: the account with the
/// lowest id is the first and stays; any other is deleted. Returns whether `user` stayed.
async fn first_user_wins(db: &mut toasty::Db, user: &User) -> Result<bool> {
    let first = User::all()
        .order_by(User::fields().id().asc())
        .first()
        .exec(db)
        .await?;
    if first.is_some_and(|first| first.id == user.id) {
        return Ok(true);
    }
    User::filter_by_id(user.id).delete().exec(db).await?;
    Ok(false)
}

/// Creates an account and logs it in, if the site's [`Registration`] setting allows.
pub async fn register(
    cx: &Cx,
    email: &str,
    password: &str,
) -> Result<std::result::Result<User, AuthError>> {
    // Every attempt counts, whatever its outcome, and is checked before any hashing.
    let throttle = context::<Throttle>(cx);
    let key = format!("register:ip:{}", ip_key(client_ip(cx)));
    let now = Instant::now();
    if throttle.is_blocked(&key, REGISTER_PER_IP, now) {
        return Ok(Err(AuthError::Throttled));
    }
    throttle.record(&key, REGISTER_PER_IP, now);

    // Refused before anything is validated or hashed.
    if !registration_open(cx).await? {
        return Ok(Err(AuthError::Closed));
    }

    let mut db = db(cx);
    let user = match create_account(&mut db, email, password).await? {
        Ok(user) => user,
        Err(error) => return Ok(Err(error)),
    };
    if *context::<Registration>(cx) == Registration::FirstUser
        && !first_user_wins(&mut db, &user).await?
    {
        return Ok(Err(AuthError::Closed));
    }

    start_session(cx, &user).await?;
    Ok(Ok(user))
}

/// Creates an account without logging anyone in and without consulting the
/// [`Registration`] setting: the checks that always apply (a plausible email, an
/// acceptable password, an email nobody uses) and nothing else. Used by sign-up, and by
/// `todo-app create-user` to make accounts on a site with sign-ups closed.
pub async fn create_account(
    db: &mut toasty::Db,
    email: &str,
    password: &str,
) -> Result<std::result::Result<User, AuthError>> {
    let Some(email) = normalize_email(email) else {
        return Ok(Err(AuthError::InvalidEmail));
    };
    if !password_is_acceptable(password) {
        return Ok(Err(AuthError::WeakPassword));
    }

    if User::filter_by_email(&email)
        .first()
        .exec(&mut *db)
        .await?
        .is_some()
    {
        return Ok(Err(AuthError::EmailTaken));
    }

    // Argon2 is CPU-heavy by design, so keep it off the async executor threads.
    let password = password.to_owned();
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|error| other_error(format!("password hashing task failed: {error}")))?
        .map_err(|error| other_error(format!("hashing the password failed: {error}")))?;

    // The unique index on `email` still decides a race between two registrations.
    let created = toasty::create!(User {
        email: email.as_str(),
        password_hash: password_hash.as_str(),
    })
    .exec(&mut *db)
    .await;
    let user = match created {
        Ok(user) => user,
        Err(_)
            if User::filter_by_email(&email)
                .first()
                .exec(&mut *db)
                .await?
                .is_some() =>
        {
            return Ok(Err(AuthError::EmailTaken));
        }
        Err(error) => return Err(error.into()),
    };

    Ok(Ok(user))
}

/// A short key for a client's network address, for rate limiting. IPv6 clients are
/// grouped by their /64 network, because one person usually controls a whole /64 and
/// could otherwise dodge the limits by rotating addresses.
fn ip_key(ip: Option<IpAddr>) -> String {
    match ip.map(|ip| ip.to_canonical()) {
        None => "unknown".to_owned(),
        Some(IpAddr::V4(v4)) => v4.to_string(),
        Some(IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// Checks credentials and logs the account in, unless this client has failed too often
/// recently. The limits are checked first, so a blocked client costs no password hashing
/// (which would otherwise be a cheap way to burn CPU). A success clears the account's
/// failure count; failures count against both the account and the network.
pub async fn log_in(
    cx: &Cx,
    email: &str,
    password: &str,
) -> Result<std::result::Result<User, AuthError>> {
    let throttle = context::<Throttle>(cx);
    let ip = ip_key(client_ip(cx));
    let account_key = format!(
        "login:account:{ip}|{}",
        normalize_email(email).unwrap_or_default()
    );
    let ip_key = format!("login:ip:{ip}");

    let now = Instant::now();
    if throttle.is_blocked(&ip_key, LOGIN_PER_IP, now)
        || throttle.is_blocked(&account_key, LOGIN_PER_ACCOUNT, now)
    {
        return Ok(Err(AuthError::Throttled));
    }

    let outcome = check_credentials(cx, email, password).await?;
    match outcome {
        Ok(_) => throttle.clear(&account_key),
        Err(_) => {
            let now = Instant::now();
            throttle.record(&ip_key, LOGIN_PER_IP, now);
            throttle.record(&account_key, LOGIN_PER_ACCOUNT, now);
        }
    }
    Ok(outcome)
}

async fn check_credentials(
    cx: &Cx,
    email: &str,
    password: &str,
) -> Result<std::result::Result<User, AuthError>> {
    // A malformed email can't belong to an account. Report it like a wrong password.
    let email = normalize_email(email).unwrap_or_default();
    if password.chars().count() > MAX_PASSWORD_LEN {
        return Ok(Err(AuthError::BadCredentials));
    }

    let mut db = db(cx);
    let user = User::filter_by_email(&email).first().exec(&mut db).await?;

    let stored = user.as_ref().map_or_else(
        || dummy_hash().to_owned(),
        |user| user.password_hash.clone(),
    );
    let password = password.to_owned();
    let matches = tokio::task::spawn_blocking(move || verify_password(&password, &stored))
        .await
        .map_err(|error| other_error(format!("password check task failed: {error}")))?;

    match user {
        Some(user) if matches => {
            start_session(cx, &user).await?;
            Ok(Ok(user))
        }
        _ => Ok(Err(AuthError::BadCredentials)),
    }
}

/// Issues a fresh session token to the client and records which user it belongs to.
async fn start_session(cx: &Cx, user: &User) -> Result<()> {
    let session: Session = session::start(cx).await?;
    toasty::create!(SessionRecord {
        token_hash: session.token_hash.to_vec(),
        user_id: user.id,
        expires_at: unix_seconds(session.expires_at),
    })
    .exec(&mut db(cx))
    .await?;
    Ok(())
}

/// Ends the current session, on the client and in the database.
pub async fn log_out(cx: &Cx) -> Result<()> {
    if let Some(hash) = session::stop(cx).await? {
        SessionRecord::filter_by_token_hash(hash.to_vec())
            .delete()
            .exec(&mut db(cx))
            .await?;
    }
    Ok(())
}

/// The logged-in user, or `None` if there is no valid, unexpired session.
pub async fn current_user(cx: &Cx) -> Result<Option<User>> {
    let Some(hash) = session::token_hash(cx).await? else {
        return Ok(None);
    };
    user_for_token(cx, &hash).await
}

async fn user_for_token(cx: &Cx, hash: &TokenHash) -> Result<Option<User>> {
    user_for_hash(
        &mut db(cx),
        hash.as_slice(),
        unix_seconds(SystemTime::now()),
    )
    .await
}

/// The user a session token hash belongs to, if the session exists and has not expired
/// by `now` (Unix seconds).
async fn user_for_hash(db: &mut toasty::Db, hash: &[u8], now: i64) -> Result<Option<User>> {
    let Some(record) = SessionRecord::filter_by_token_hash(hash.to_vec())
        .first()
        .exec(db)
        .await?
    else {
        return Ok(None);
    };
    if record.expires_at <= now {
        return Ok(None);
    }
    Ok(User::filter_by_id(record.user_id).first().exec(db).await?)
}

/// Deletes every session that has expired by `now` (Unix seconds). Expired sessions
/// already can't log anyone in; this only keeps the table from growing forever.
pub async fn purge_expired_sessions(db: &mut toasty::Db, now: i64) -> Result<()> {
    SessionRecord::filter(SessionRecord::fields().expires_at().le(now))
        .delete()
        .exec(db)
        .await?;
    Ok(())
}

/// Runs [`purge_expired_sessions`], and clears expired password-reset links, now and then
/// once an hour, forever. Spawn it once at startup. A failed sweep is reported and retried
/// at the next interval.
pub async fn sweep_expired_sessions(mut db: toasty::Db) {
    loop {
        let now = unix_seconds(SystemTime::now());
        if let Err(error) = purge_expired_sessions(&mut db, now).await {
            eprintln!("could not delete expired sessions: {error:#}");
        }
        if let Err(error) = super::reset::purge_expired(&mut db, now).await {
            eprintln!("could not delete expired reset links: {error:#}");
        }
        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    }
}

/// Whether a session expiring at `expires_at` is due for a fresh lifetime at `now`
/// (Unix seconds): once less than half of it is left. Refreshing on every request would
/// rewrite the cookie and a row each time; refreshing at half-life keeps active
/// sessions alive with about two writes per lifetime.
fn should_refresh(expires_at: i64, now: i64) -> bool {
    let half = i64::try_from(SESSION_LIFETIME.as_secs() / 2).unwrap_or(i64::MAX);
    expires_at - now <= half
}

/// Extends the current session if it is more than half used. Call it early in a page
/// handler, before the response starts, because it sets a cookie.
pub async fn refresh_session(cx: &Cx) -> Result<()> {
    let Some(hash) = session::token_hash(cx).await? else {
        return Ok(());
    };
    let mut db = db(cx);
    let Some(mut record) = SessionRecord::filter_by_token_hash(hash.to_vec())
        .first()
        .exec(&mut db)
        .await?
    else {
        return Ok(());
    };
    let now = unix_seconds(SystemTime::now());
    if record.expires_at <= now || !should_refresh(record.expires_at, now) {
        return Ok(());
    }
    if let Some(session) = session::refresh(cx).await? {
        record
            .update()
            .expires_at(unix_seconds(session.expires_at))
            .exec(&mut db)
            .await?;
    }
    Ok(())
}

/// Like [`current_user`], but redirects to the login page when nobody is logged in.
pub async fn require_user(cx: &Cx) -> Result<User> {
    use topcoat::router::{error::RouterErrorExt, href};
    let user = current_user(cx)
        .await?
        .ok_or_redirect(href!(crate::app::login::page).resolve(cx))?;
    Ok(user)
}

// --- Changing the password ---------------------------------------------------------

/// Why a password change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordError {
    /// The "current password" given is wrong.
    WrongCurrent,
    /// The new password is too short or too long.
    WeakPassword,
    /// The new password and its confirmation differ.
    Mismatch,
    /// The new password is the same as the current one.
    SameAsCurrent,
    /// Too many wrong current-password answers recently.
    Throttled,
}

impl PasswordError {
    /// The message shown on the form.
    pub fn message(self) -> &'static str {
        match self {
            Self::WrongCurrent => "The current password is not correct.",
            Self::WeakPassword => "Use a new password of 12 to 128 characters.",
            Self::Mismatch => "The new password and its confirmation do not match.",
            Self::SameAsCurrent => "Choose a new password that is different from the current one.",
            Self::Throttled => "Too many attempts. Please wait a few minutes and try again.",
        }
    }

    /// A short code carried in the redirect back to the form.
    pub fn code(self) -> &'static str {
        match self {
            Self::WrongCurrent => "current",
            Self::WeakPassword => "weak",
            Self::Mismatch => "mismatch",
            Self::SameAsCurrent => "same",
            Self::Throttled => "throttled",
        }
    }

    /// The inverse of [`PasswordError::code`].
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::WrongCurrent,
            Self::WeakPassword,
            Self::Mismatch,
            Self::SameAsCurrent,
            Self::Throttled,
        ]
        .into_iter()
        .find(|error| error.code() == code)
    }
}

/// The rate-limit key for wrong current-password answers about one account.
fn password_user_key(user_id: u64) -> String {
    format!("password:user:{user_id}")
}

/// Checks `current` against the account's password and returns the account if it matches.
///
/// Every place that asks for the current password (changing it, deleting the account) goes
/// through here and shares one set of limits, so using several of them cannot multiply the
/// number of guesses. Wrong answers are limited per account and per network, and the limits
/// are checked first, so a blocked client costs no password hashing. A correct answer does
/// not reset the counts; the caller decides whether it should.
pub async fn verify_current_password(
    cx: &Cx,
    user_id: u64,
    current: &str,
) -> Result<std::result::Result<User, PasswordError>> {
    let throttle = context::<Throttle>(cx);
    let user_key = password_user_key(user_id);
    let ip_key = format!("password:ip:{}", ip_key(client_ip(cx)));
    let now = Instant::now();
    if throttle.is_blocked(&user_key, PASSWORD_PER_ACCOUNT, now)
        || throttle.is_blocked(&ip_key, PASSWORD_PER_IP, now)
    {
        return Ok(Err(PasswordError::Throttled));
    }

    let user = User::filter_by_id(user_id)
        .first()
        .exec(&mut db(cx))
        .await?;
    let matches = match (&user, current.chars().count() <= MAX_PASSWORD_LEN) {
        (Some(user), true) => {
            let (current, stored) = (current.to_owned(), user.password_hash.clone());
            tokio::task::spawn_blocking(move || verify_password(&current, &stored))
                .await
                .map_err(|error| other_error(format!("password check task failed: {error}")))?
        }
        _ => false,
    };
    match user.filter(|_| matches) {
        Some(user) => Ok(Ok(user)),
        None => {
            let now = Instant::now();
            throttle.record(&user_key, PASSWORD_PER_ACCOUNT, now);
            throttle.record(&ip_key, PASSWORD_PER_IP, now);
            Ok(Err(PasswordError::WrongCurrent))
        }
    }
}

/// Checks the current password and, if the new one is acceptable, stores it. This does not
/// touch sessions; [`change_password`] does that afterwards.
pub async fn verify_and_set(
    cx: &Cx,
    user_id: u64,
    current: &str,
    new: &str,
) -> Result<std::result::Result<(), PasswordError>> {
    // A bad new password is a form mistake, not a guess at the current one, so it is
    // reported without counting against the limits.
    if !password_is_acceptable(new) {
        return Ok(Err(PasswordError::WeakPassword));
    }

    let mut user = match verify_current_password(cx, user_id, current).await? {
        Ok(user) => user,
        Err(error) => return Ok(Err(error)),
    };

    // Only said once the person has proved they know the current password.
    if new == current {
        return Ok(Err(PasswordError::SameAsCurrent));
    }

    let new = new.to_owned();
    let new_hash = tokio::task::spawn_blocking(move || hash_password(&new))
        .await
        .map_err(|error| other_error(format!("password hashing task failed: {error}")))?
        .map_err(|error| other_error(format!("hashing the password failed: {error}")))?;
    user.update()
        .password_hash(new_hash.as_str())
        .exec(&mut db(cx))
        .await?;

    context::<Throttle>(cx).clear(&password_user_key(user_id));
    Ok(Ok(()))
}

/// Deletes every session of the user, then records `keep` (a token hash and its expiry) as
/// the only one left, if given.
async fn replace_sessions(
    db: &mut toasty::Db,
    user_id: u64,
    keep: Option<(Vec<u8>, i64)>,
) -> Result<()> {
    SessionRecord::filter_by_user_id(user_id)
        .delete()
        .exec(db)
        .await?;
    if let Some((token_hash, expires_at)) = keep {
        toasty::create!(SessionRecord {
            token_hash,
            user_id,
            expires_at,
        })
        .exec(db)
        .await?;
    }
    Ok(())
}

/// Changes the logged-in user's password.
///
/// On success every other session of the account is revoked, so anyone who had stolen one
/// is logged out, and the current session gets a fresh token (Topcoat's `rotate`, for a
/// change of privileges), so the person stays logged in.
pub async fn change_password(
    cx: &Cx,
    user_id: u64,
    current: &str,
    new: &str,
) -> Result<std::result::Result<(), PasswordError>> {
    if let Err(error) = verify_and_set(cx, user_id, current, new).await? {
        return Ok(Err(error));
    }
    let keep = session::rotate(cx).await?.map(|rotation| {
        (
            rotation.session.token_hash.to_vec(),
            unix_seconds(rotation.session.expires_at),
        )
    });
    replace_sessions(&mut db(cx), user_id, keep).await?;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_is_trimmed_and_lowercased() {
        assert_eq!(
            normalize_email("  Dana@Example.COM "),
            Some("dana@example.com".to_owned())
        );
    }

    #[test]
    fn implausible_emails_are_rejected() {
        for bad in [
            "",
            "dana",
            "@example.com",
            "dana@",
            "dana@example",
            "a b@example.com",
            "a@@b.com",
            "a@.com",
            "a@b.",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad:?}");
        }
        let long = format!("{}@example.com", "a".repeat(MAX_EMAIL_LEN));
        assert_eq!(normalize_email(&long), None);
    }

    #[test]
    fn password_length_bounds() {
        assert!(!password_is_acceptable(&"a".repeat(MIN_PASSWORD_LEN - 1)));
        assert!(password_is_acceptable(&"a".repeat(MIN_PASSWORD_LEN)));
        assert!(password_is_acceptable(&"a".repeat(MAX_PASSWORD_LEN)));
        assert!(!password_is_acceptable(&"a".repeat(MAX_PASSWORD_LEN + 1)));
        // Characters, not bytes: 12 two-byte characters are long enough.
        assert!(password_is_acceptable(&"é".repeat(12)));
    }

    #[test]
    fn hashes_verify_and_are_salted() {
        let first = hash_password("correct horse battery").unwrap();
        let second = hash_password("correct horse battery").unwrap();
        assert_ne!(first, second, "each hash has its own salt");
        assert!(first.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &first));
        assert!(!verify_password("wrong horse battery", &first));
        assert!(!verify_password("anything", "not a hash"));
    }

    #[test]
    fn error_codes_round_trip() {
        for error in [
            AuthError::InvalidEmail,
            AuthError::WeakPassword,
            AuthError::EmailTaken,
            AuthError::BadCredentials,
            AuthError::Throttled,
            AuthError::Closed,
        ] {
            assert_eq!(AuthError::from_code(error.code()), Some(error));
        }
        assert_eq!(AuthError::from_code("nonsense"), None);
    }

    // --- Rate limiting, through the real log_in and register ---

    async fn create_user(cx: &Cx, email: &str, password: &str) -> User {
        let password_hash = hash_password(password).expect("hashes");
        toasty::create!(User {
            email,
            password_hash: password_hash.as_str(),
        })
        .exec(&mut db(cx))
        .await
        .expect("user created")
    }

    async fn failed_login(cx: &Cx, email: &str) -> Option<AuthError> {
        log_in(cx, email, "definitely the wrong password")
            .await
            .expect("no internal error")
            .err()
    }

    #[tokio::test]
    async fn repeated_failures_block_even_the_right_password() {
        let cx = crate::db::test_cx().await;
        create_user(&cx, "alice@example.com", "correct horse battery").await;

        for attempt in 1..=5 {
            assert_eq!(
                failed_login(&cx, "alice@example.com").await,
                Some(AuthError::BadCredentials),
                "attempt {attempt}"
            );
        }
        // Refused before the password is even checked, so a lucky guess doesn't help.
        let outcome = log_in(&cx, "alice@example.com", "correct horse battery")
            .await
            .expect("no internal error");
        assert_eq!(outcome.err(), Some(AuthError::Throttled));
    }

    #[tokio::test]
    async fn failures_for_an_unknown_email_count_too() {
        let cx = crate::db::test_cx().await;
        for _ in 0..5 {
            assert_eq!(
                failed_login(&cx, "nobody@example.com").await,
                Some(AuthError::BadCredentials)
            );
        }
        assert_eq!(
            failed_login(&cx, "nobody@example.com").await,
            Some(AuthError::Throttled),
            "no difference between known and unknown emails"
        );
    }

    #[tokio::test]
    async fn blocking_one_account_does_not_block_another() {
        let cx = crate::db::test_cx().await;
        create_user(&cx, "alice@example.com", "correct horse battery").await;
        create_user(&cx, "bob@example.com", "another long password").await;
        for _ in 0..5 {
            failed_login(&cx, "alice@example.com").await;
        }
        assert_eq!(
            failed_login(&cx, "alice@example.com").await,
            Some(AuthError::Throttled)
        );
        assert_eq!(
            failed_login(&cx, "bob@example.com").await,
            Some(AuthError::BadCredentials),
            "Bob is judged on his own failures"
        );
    }

    #[tokio::test]
    async fn the_network_limit_blocks_every_account_from_that_network() {
        let cx = crate::db::test_cx().await;
        let throttle = context::<Throttle>(&cx);
        let now = Instant::now();
        for _ in 0..LOGIN_PER_IP.max {
            throttle.record("login:ip:unknown", LOGIN_PER_IP, now);
        }
        assert_eq!(
            failed_login(&cx, "fresh-account@example.com").await,
            Some(AuthError::Throttled)
        );
    }

    #[tokio::test]
    async fn registration_attempts_are_limited_per_network() {
        let cx = crate::db::test_cx().await;
        for attempt in 1..=REGISTER_PER_IP.max {
            let outcome = register(&cx, "not-an-email", "irrelevant password")
                .await
                .expect("no internal error");
            assert_eq!(
                outcome.err(),
                Some(AuthError::InvalidEmail),
                "attempt {attempt}"
            );
        }
        let outcome = register(&cx, "not-an-email", "irrelevant password")
            .await
            .expect("no internal error");
        assert_eq!(outcome.err(), Some(AuthError::Throttled));
    }

    #[test]
    fn network_keys_group_ipv6_by_64_and_unwrap_mapped_ipv4() {
        let key = |text: &str| ip_key(Some(text.parse().unwrap()));
        assert_eq!(ip_key(None), "unknown");
        assert_eq!(key("203.0.113.9"), "203.0.113.9");
        assert_eq!(key("::ffff:203.0.113.9"), "203.0.113.9");
        assert_eq!(
            key("2001:db8:1:2:aaaa::1"),
            key("2001:db8:1:2:bbbb:cccc:dddd:eeee"),
            "one /64 is one client"
        );
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
    }

    // --- Who may sign up ---

    async fn user_count(cx: &Cx) -> usize {
        User::all().exec(&mut db(cx)).await.unwrap().len()
    }

    async fn register_outcome(cx: &Cx, email: &str, password: &str) -> Option<AuthError> {
        register(cx, email, password)
            .await
            .expect("no internal error")
            .err()
    }

    #[tokio::test]
    async fn open_mode_lets_anyone_sign_up_and_still_validates() {
        let cx = crate::db::test_cx().await;
        assert!(registration_open(&cx).await.unwrap());
        assert_eq!(
            register_outcome(&cx, "not-an-email", "a long enough password").await,
            Some(AuthError::InvalidEmail)
        );
        assert_eq!(
            register_outcome(&cx, "a@example.com", "short").await,
            Some(AuthError::WeakPassword)
        );
    }

    #[tokio::test]
    async fn closed_mode_refuses_everyone_before_anything_is_checked_or_stored() {
        let cx = crate::db::test_cx().await.with(Registration::Closed);
        assert!(!registration_open(&cx).await.unwrap());
        // Even a perfectly valid sign-up is refused, and nothing is created.
        assert_eq!(
            register_outcome(&cx, "a@example.com", "a long enough password").await,
            Some(AuthError::Closed)
        );
        assert_eq!(user_count(&cx).await, 0);
        // Invalid input gets the same answer: a closed site does not coach anyone.
        assert_eq!(
            register_outcome(&cx, "nonsense", "x").await,
            Some(AuthError::Closed)
        );
        assert_eq!(user_count(&cx).await, 0);
    }

    #[tokio::test]
    async fn closed_mode_does_not_affect_logging_in() {
        let cx = crate::db::test_cx().await.with(Registration::Closed);
        create_user(&cx, "a@example.com", OLD).await;
        // A wrong password is a wrong password, not "closed".
        assert_eq!(
            failed_login(&cx, "a@example.com").await,
            Some(AuthError::BadCredentials)
        );
    }

    #[tokio::test]
    async fn first_user_mode_is_open_only_until_an_account_exists() {
        let cx = crate::db::test_cx().await.with(Registration::FirstUser);
        assert!(registration_open(&cx).await.unwrap(), "no accounts yet");

        let mut handle = db(&cx);
        create_account(&mut handle, "owner@example.com", OLD)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !registration_open(&cx).await.unwrap(),
            "the first account closes it"
        );
        assert_eq!(
            register_outcome(&cx, "later@example.com", "a long enough password").await,
            Some(AuthError::Closed)
        );
        assert_eq!(user_count(&cx).await, 1, "the late comer was not created");
    }

    #[tokio::test]
    async fn when_two_first_sign_ups_race_only_the_earliest_survives() {
        let cx = crate::db::test_cx().await.with(Registration::FirstUser);
        let mut handle = db(&cx);
        // Both passed the "no accounts yet" check before either was saved.
        let first = create_account(&mut handle, "one@example.com", OLD)
            .await
            .unwrap()
            .unwrap();
        let second = create_account(&mut handle, "two@example.com", OLD)
            .await
            .unwrap()
            .unwrap();
        assert!(first.id < second.id);

        assert!(
            !first_user_wins(&mut handle, &second).await.unwrap(),
            "the later one loses"
        );
        assert_eq!(user_count(&cx).await, 1, "and is removed");
        assert!(
            first_user_wins(&mut handle, &first).await.unwrap(),
            "the earlier one stays"
        );
        assert_eq!(user_count(&cx).await, 1);
    }

    #[tokio::test]
    async fn create_account_always_applies_the_basic_checks_whatever_the_mode() {
        let cx = crate::db::test_cx().await.with(Registration::Closed);
        let mut handle = db(&cx);
        let outcome = |r: Result<std::result::Result<User, AuthError>>| r.unwrap().err();

        assert_eq!(
            outcome(create_account(&mut handle, "nonsense", OLD).await),
            Some(AuthError::InvalidEmail)
        );
        assert_eq!(
            outcome(create_account(&mut handle, "a@example.com", "short").await),
            Some(AuthError::WeakPassword)
        );

        let user = create_account(&mut handle, " Admin@Example.COM ", OLD)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.email, "admin@example.com", "normalized");
        assert!(user.password_hash.starts_with("$argon2id$"));
        assert!(verify_password(OLD, &user.password_hash));
        assert_eq!(
            outcome(create_account(&mut handle, "ADMIN@example.com", OLD).await),
            Some(AuthError::EmailTaken)
        );
        assert_eq!(user_count(&cx).await, 1);
    }

    // --- Changing the password ---

    const OLD: &str = "old password 1234";
    const NEW: &str = "brand new password 9";

    async fn stored_hash(cx: &Cx, user_id: u64) -> String {
        User::filter_by_id(user_id)
            .first()
            .exec(&mut db(cx))
            .await
            .unwrap()
            .expect("user exists")
            .password_hash
    }

    async fn change(
        cx: &Cx,
        user_id: u64,
        current: &str,
        new: &str,
    ) -> std::result::Result<(), PasswordError> {
        verify_and_set(cx, user_id, current, new)
            .await
            .expect("no internal error")
    }

    #[tokio::test]
    async fn the_correct_current_password_sets_the_new_one() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "a@example.com", OLD).await;
        assert_eq!(change(&cx, user.id, OLD, NEW).await, Ok(()));

        let hash = stored_hash(&cx, user.id).await;
        assert!(
            hash.starts_with("$argon2id$"),
            "stored as a salted Argon2id hash"
        );
        assert!(verify_password(NEW, &hash));
        assert!(
            !verify_password(OLD, &hash),
            "the old password no longer works"
        );
        assert!(!hash.contains(NEW), "never stored in the clear");
    }

    #[tokio::test]
    async fn a_wrong_current_password_changes_nothing() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "a@example.com", OLD).await;
        let before = stored_hash(&cx, user.id).await;

        assert_eq!(
            change(&cx, user.id, "not the password", NEW).await,
            Err(PasswordError::WrongCurrent)
        );
        assert_eq!(
            change(&cx, user.id, "", NEW).await,
            Err(PasswordError::WrongCurrent)
        );
        assert_eq!(stored_hash(&cx, user.id).await, before);
    }

    #[tokio::test]
    async fn an_unknown_account_or_an_oversized_answer_is_just_a_wrong_password() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "a@example.com", OLD).await;
        assert_eq!(
            change(&cx, 987_654, OLD, NEW).await,
            Err(PasswordError::WrongCurrent)
        );
        let huge = "x".repeat(MAX_PASSWORD_LEN + 1);
        assert_eq!(
            change(&cx, user.id, &huge, NEW).await,
            Err(PasswordError::WrongCurrent)
        );
    }

    #[tokio::test]
    async fn a_bad_new_password_is_refused_without_counting_as_a_guess() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "a@example.com", OLD).await;
        let before = stored_hash(&cx, user.id).await;

        // Far more than the limit of 5, yet none of them is a guess at the current password.
        for _ in 0..8 {
            assert_eq!(
                change(&cx, user.id, OLD, "short").await,
                Err(PasswordError::WeakPassword)
            );
        }
        let too_long = "x".repeat(MAX_PASSWORD_LEN + 1);
        assert_eq!(
            change(&cx, user.id, OLD, &too_long).await,
            Err(PasswordError::WeakPassword)
        );
        assert_eq!(stored_hash(&cx, user.id).await, before);

        assert_eq!(
            change(&cx, user.id, OLD, NEW).await,
            Ok(()),
            "not locked out by form mistakes"
        );
    }

    #[tokio::test]
    async fn the_new_password_must_differ_but_only_a_correct_current_one_learns_that() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "a@example.com", OLD).await;
        assert_eq!(
            change(&cx, user.id, OLD, OLD).await,
            Err(PasswordError::SameAsCurrent)
        );
        // Someone who does not know the password is told only that it is wrong.
        assert_eq!(
            change(
                &cx,
                user.id,
                "guess that is long enough",
                "guess that is long enough"
            )
            .await,
            Err(PasswordError::WrongCurrent)
        );
    }

    #[tokio::test]
    async fn guessing_the_current_password_is_rate_limited_even_for_the_right_answer() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "a@example.com", OLD).await;
        let before = stored_hash(&cx, user.id).await;

        for attempt in 1..=5 {
            assert_eq!(
                change(&cx, user.id, "wrong guess", NEW).await,
                Err(PasswordError::WrongCurrent),
                "attempt {attempt}"
            );
        }
        assert_eq!(
            change(&cx, user.id, OLD, NEW).await,
            Err(PasswordError::Throttled)
        );
        assert_eq!(
            stored_hash(&cx, user.id).await,
            before,
            "blocked before anything was written"
        );
    }

    #[tokio::test]
    async fn the_limit_is_per_account_and_a_success_resets_it() {
        let cx = crate::db::test_cx().await;
        let alice = create_user(&cx, "a@example.com", OLD).await;
        let bob = create_user(&cx, "b@example.com", OLD).await;

        for _ in 0..5 {
            let _ = change(&cx, alice.id, "wrong guess", NEW).await;
        }
        assert_eq!(
            change(&cx, alice.id, OLD, NEW).await,
            Err(PasswordError::Throttled)
        );
        assert_eq!(
            change(&cx, bob.id, OLD, NEW).await,
            Ok(()),
            "Bob is judged on his own failures"
        );

        // Four wrong answers then a right one: the count starts again from zero.
        let carol = create_user(&cx, "c@example.com", OLD).await;
        for _ in 0..4 {
            assert_eq!(
                change(&cx, carol.id, "wrong guess", NEW).await,
                Err(PasswordError::WrongCurrent)
            );
        }
        assert_eq!(change(&cx, carol.id, OLD, NEW).await, Ok(()));
        for _ in 0..4 {
            assert_eq!(
                change(&cx, carol.id, "wrong guess", "another new password 7").await,
                Err(PasswordError::WrongCurrent),
                "not throttled: the earlier failures were cleared by the success"
            );
        }
    }

    #[tokio::test]
    async fn replacing_sessions_keeps_only_the_new_one_and_only_for_that_user() {
        let cx = crate::db::test_cx().await;
        let alice = create_user(&cx, "a@example.com", OLD).await;
        let bob = create_user(&cx, "b@example.com", OLD).await;
        add_session(&cx, [1; 32], alice.id, 9_999_999_999).await; // her old token
        add_session(&cx, [2; 32], alice.id, 9_999_999_999).await; // a thief's session
        add_session(&cx, [3; 32], bob.id, 9_999_999_999).await;

        replace_sessions(&mut db(&cx), alice.id, Some((vec![9; 32], 9_999_999_999)))
            .await
            .unwrap();
        assert!(
            !session_exists(&cx, [1; 32]).await,
            "the old token is revoked"
        );
        assert!(
            !session_exists(&cx, [2; 32]).await,
            "another device's session is revoked"
        );
        assert!(
            session_exists(&cx, [9; 32]).await,
            "the rotated session is recorded"
        );
        assert!(
            session_exists(&cx, [3; 32]).await,
            "other users are untouched"
        );

        // The new token finds her; the old ones find nobody.
        let mut handle = db(&cx);
        assert!(
            user_for_hash(&mut handle, &[9; 32], 0)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            user_for_hash(&mut handle, &[2; 32], 0)
                .await
                .unwrap()
                .is_none()
        );

        replace_sessions(&mut db(&cx), alice.id, None)
            .await
            .unwrap();
        assert!(
            !session_exists(&cx, [9; 32]).await,
            "with nothing to keep, all are dropped"
        );
        assert!(session_exists(&cx, [3; 32]).await);
    }

    #[test]
    fn password_error_codes_round_trip() {
        for error in [
            PasswordError::WrongCurrent,
            PasswordError::WeakPassword,
            PasswordError::Mismatch,
            PasswordError::SameAsCurrent,
            PasswordError::Throttled,
        ] {
            assert_eq!(PasswordError::from_code(error.code()), Some(error));
            assert!(!error.message().is_empty());
        }
        assert_eq!(PasswordError::from_code("nope"), None);
    }

    // --- Sessions: lookup, expiry, cleanup, refresh ---

    async fn add_session(cx: &Cx, token_hash: [u8; 32], user_id: u64, expires_at: i64) {
        toasty::create!(SessionRecord {
            token_hash: token_hash.to_vec(),
            user_id,
            expires_at,
        })
        .exec(&mut db(cx))
        .await
        .expect("session recorded");
    }

    async fn session_exists(cx: &Cx, token_hash: [u8; 32]) -> bool {
        SessionRecord::filter_by_token_hash(token_hash.to_vec())
            .first()
            .exec(&mut db(cx))
            .await
            .expect("queried")
            .is_some()
    }

    #[tokio::test]
    async fn a_session_finds_its_user_until_it_expires() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "alice@example.com", "correct horse battery").await;
        add_session(&cx, [7; 32], user.id, 1_000).await;
        let mut handle = db(&cx);

        let found = user_for_hash(&mut handle, &[7; 32], 999).await.unwrap();
        assert_eq!(found.map(|u| u.email), Some("alice@example.com".to_owned()));
        assert!(
            user_for_hash(&mut handle, &[7; 32], 1_000)
                .await
                .unwrap()
                .is_none(),
            "expired at the expiry second"
        );
        assert!(
            user_for_hash(&mut handle, &[7; 32], 5_000)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_unknown_or_different_token_finds_nobody() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "alice@example.com", "correct horse battery").await;
        add_session(&cx, [7; 32], user.id, 1_000).await;
        let mut handle = db(&cx);

        assert!(
            user_for_hash(&mut handle, &[8; 32], 0)
                .await
                .unwrap()
                .is_none()
        );
        assert!(user_for_hash(&mut handle, &[], 0).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_session_whose_user_is_gone_finds_nobody() {
        let cx = crate::db::test_cx().await;
        add_session(&cx, [9; 32], 12_345, 1_000).await;
        assert!(
            user_for_hash(&mut db(&cx), &[9; 32], 0)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn purging_deletes_expired_sessions_and_keeps_live_ones() {
        let cx = crate::db::test_cx().await;
        let user = create_user(&cx, "alice@example.com", "correct horse battery").await;
        add_session(&cx, [1; 32], user.id, 100).await; // long expired
        add_session(&cx, [2; 32], user.id, 1_000).await; // expires exactly now
        add_session(&cx, [3; 32], user.id, 5_000).await; // still live

        purge_expired_sessions(&mut db(&cx), 1_000).await.unwrap();
        assert!(!session_exists(&cx, [1; 32]).await);
        assert!(!session_exists(&cx, [2; 32]).await);
        assert!(
            session_exists(&cx, [3; 32]).await,
            "a live session survives"
        );

        // Running it again changes nothing.
        purge_expired_sessions(&mut db(&cx), 1_000).await.unwrap();
        assert!(session_exists(&cx, [3; 32]).await);
    }

    #[test]
    fn sessions_refresh_once_half_their_life_is_gone() {
        let day = 24 * 60 * 60;
        let now = 1_000_000;
        // 14-day lifetime: refresh when 7 days or fewer remain.
        assert!(!should_refresh(now + 14 * day, now), "just issued");
        assert!(!should_refresh(now + 8 * day, now));
        assert!(should_refresh(now + 7 * day, now), "exactly half");
        assert!(should_refresh(now + day, now));
        assert!(
            should_refresh(now - 1, now),
            "already expired (callers check separately)"
        );
    }
}
