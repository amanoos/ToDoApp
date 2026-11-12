//! Settings read from environment variables when the app starts.
//!
//! Anything that looks like a typo is an error, so a deployment fails at startup with a
//! clear message instead of quietly running with the wrong setting. (Topcoat's own
//! `TrustedProxies::networks` would panic on a bad address; validating here first turns
//! that into a readable error.)
//!
//! | Variable | Default | Meaning |
//! | --- | --- | --- |
//! | `DATABASE_URL` | the local dev container | SQL Server, e.g. `sqlserver://sa:pw@db:1433/todoapp` |
//! | `MIGRATE_ON_START` | `true` | apply pending database migrations at startup |
//! | `REGISTRATION` | `open` | who may sign up: `open`, `closed` or `first-user` |
//! | `PUBLIC_URL` | none | the site's public address, for full links in the command line tools |
//! | `SMTP_URL` | none | mail server for emailed password reset links, e.g. `smtps://user:pw@host:465` |
//! | `MAIL_FROM` | none | sender of those emails (required with `SMTP_URL`, as is `PUBLIC_URL`) |
//! | `TRUSTED_PROXIES` | none | comma-separated addresses or CIDR networks of reverse proxies |
//! | `TRUSTED_PROXY_HOPS` | `0` | number of reverse proxies in front, for when their addresses vary |
//!
//! `HOST`, `PORT` (read by Topcoat) and the security settings in `security.rs` are
//! documented in `docs/DEPLOYMENT.md`.

use std::{fmt, net::IpAddr};

/// Where the database lives when `DATABASE_URL` is not set: the throwaway SQL Server
/// container that `docker-compose.dev.yml` starts on this machine. Its password is public on
/// purpose; the container only listens on 127.0.0.1. Never rely on this default in production.
pub const DEFAULT_DATABASE_URL: &str =
    "sqlserver://sa:ToDoApp-Dev-Passw0rd@127.0.0.1:1433/todoapp?trust_server_certificate=true";

/// The most reverse-proxy hops accepted; more than a handful is a mistake.
const MAX_PROXY_HOPS: usize = 8;

/// A setting that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    variable: &'static str,
    problem: String,
}

impl ConfigError {
    fn new(variable: &'static str, problem: impl Into<String>) -> Self {
        Self {
            variable,
            problem: problem.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.variable, self.problem)
    }
}

impl std::error::Error for ConfigError {}

/// Who may create an account through the sign-up page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Registration {
    /// Anyone who can reach the site.
    #[default]
    Open,
    /// Nobody. Accounts are created with `todo-app create-user`.
    Closed,
    /// Anyone, until the first account exists; after that, nobody. The first person to
    /// reach a fresh site claims it, so use this only when you register straight away.
    FirstUser,
}

impl Registration {
    /// Parses the setting's value, ignoring case.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            "first-user" => Some(Self::FirstUser),
            _ => None,
        }
    }
}

/// Accepts a public address such as `https://todo.example.com` (scheme and host, optional
/// port, optionally a trailing `/`), and returns it without the trailing slash. Anything
/// with a path, query, spaces or odd characters is refused, because the address is pasted
/// into links people are asked to open.
fn parse_public_url(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    let origin = crate::security::origin_of(trimmed)?;
    (origin == trimmed).then_some(origin)
}

/// The host part of `scheme://[user:pass@]host[:port][/path][?query]`, lower-cased, without
/// brackets around an IPv6 address. `None` when there is no host or the text holds spaces
/// or control characters.
fn host_of(url: &str) -> Option<String> {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let hostport = authority.rsplit('@').next()?;
    let host = match hostport.strip_prefix('[') {
        Some(inner) => inner.split(']').next()?,
        None => hostport.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether `host` is this machine: `localhost` or a loopback address.
fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Why an `SMTP_URL` cannot be used, or `None` if it is fine. It never quotes the URL,
/// which usually holds the mail account's password.
///
/// Mail carries password reset links, so the connection must be encrypted: `smtps://`
/// (implicit TLS), or `smtp://` with `?tls=required` (STARTTLS that cannot be downgraded).
/// Plain `smtp://` is accepted only for this machine, to try the app against a local mail
/// catcher.
fn smtp_url_problem(url: &str) -> Option<&'static str> {
    let lower = url.trim().to_ascii_lowercase();
    let encrypted = if lower.starts_with("smtps://") {
        true
    } else if lower.starts_with("smtp://") {
        lower
            .split_once('?')
            .is_some_and(|(_, query)| query.split('&').any(|part| part == "tls=required"))
    } else {
        return Some("must start with smtps:// or smtp://");
    };
    let Some(host) = host_of(&lower) else {
        return Some("is not a valid SMTP address (no host, or it contains spaces)");
    };
    if !encrypted && !is_loopback(&host) {
        return Some(
            "would send mail unencrypted. Use smtps://..., or smtp://...?tls=required \
             (plain smtp:// is only allowed for localhost)",
        );
    }
    None
}

/// Settings for sending email. Present only when `SMTP_URL` is set.
#[derive(Clone, PartialEq, Eq)]
pub struct EmailConfig {
    /// Where to submit mail. Holds the mail account's password, so it is never printed.
    pub smtp_url: String,
    /// The sender shown on every message, such as `Todo <noreply@example.com>`.
    pub from: String,
}

impl fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmailConfig")
            .field("smtp_url", &"<redacted>")
            .field("from", &self.from)
            .finish()
    }
}

/// The application's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_url: String,
    pub migrate_on_start: bool,
    pub registration: Registration,
    /// The site's public address, used to write full links (such as password reset links)
    /// in the command line tools. `None` means links are written without it.
    pub public_url: Option<String>,
    /// How to send email. `None` means the site cannot send any, and offers no "forgot your
    /// password?" form.
    pub email: Option<EmailConfig>,
    /// Addresses or CIDR networks of trusted reverse proxies; already validated.
    pub trusted_proxies: Vec<String>,
    pub trusted_proxy_hops: usize,
}

/// Parses a yes/no setting.
fn parse_flag(variable: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        other => Err(ConfigError::new(
            variable,
            format!("expected true or false, got {other:?}"),
        )),
    }
}

/// Whether `item` is an IP address or a CIDR network such as `10.0.0.0/8`.
fn is_address_or_network(item: &str) -> bool {
    match item.split_once('/') {
        None => item.parse::<IpAddr>().is_ok(),
        Some((address, prefix)) => {
            let Ok(address) = address.parse::<IpAddr>() else {
                return false;
            };
            let longest = if address.is_ipv4() { 32 } else { 128 };
            // `u8::from_str` accepts a leading `+`; a prefix length should not.
            !prefix.starts_with('+') && prefix.parse::<u8>().is_ok_and(|p| p <= longest)
        }
    }
}

impl Config {
    /// Reads the settings from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(&|name| std::env::var(name).ok())
    }

    /// Reads the settings from `get`, which looks a variable up by name. Empty values are
    /// treated as unset, so `DATABASE_URL=` in a compose file means "use the default".
    pub fn from_vars(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |name: &str| get(name).filter(|value| !value.trim().is_empty());

        let database_url = get("DATABASE_URL")
            .map(|url| url.trim().to_owned())
            .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_owned());
        // The value is not echoed: it holds a password.
        if let Err(problem) = crate::db::Target::parse(&database_url) {
            return Err(ConfigError::new("DATABASE_URL", problem));
        }

        let migrate_on_start = match get("MIGRATE_ON_START") {
            Some(value) => parse_flag("MIGRATE_ON_START", &value)?,
            None => true,
        };

        let registration = match get("REGISTRATION") {
            None => Registration::default(),
            Some(value) => Registration::parse(&value).ok_or_else(|| {
                ConfigError::new(
                    "REGISTRATION",
                    format!("expected open, closed or first-user, got {value:?}"),
                )
            })?,
        };

        let public_url = match get("PUBLIC_URL") {
            None => None,
            Some(value) => Some(parse_public_url(&value).ok_or_else(|| {
                ConfigError::new(
                    "PUBLIC_URL",
                    format!(
                        "expected an address like https://todo.example.com (no path), got {value:?}"
                    ),
                )
            })?),
        };

        let email = match (get("SMTP_URL"), get("MAIL_FROM")) {
            (None, None) => None,
            (None, Some(_)) => {
                return Err(ConfigError::new(
                    "MAIL_FROM",
                    "is set but SMTP_URL is not, so no email could be sent",
                ));
            }
            (Some(smtp_url), from) => {
                let smtp_url = smtp_url.trim().to_owned();
                if let Some(problem) = smtp_url_problem(&smtp_url) {
                    return Err(ConfigError::new("SMTP_URL", problem));
                }
                let Some(from) = from else {
                    return Err(ConfigError::new(
                        "MAIL_FROM",
                        "is required when SMTP_URL is set, for example Todo <noreply@example.com>",
                    ));
                };
                let from = from.trim().to_owned();
                if from.parse::<topcoat::mail::Mailbox>().is_err() {
                    return Err(ConfigError::new(
                        "MAIL_FROM",
                        format!(
                            "expected an address like noreply@example.com or \
                             Todo <noreply@example.com>, got {from:?}"
                        ),
                    ));
                }
                // The emailed link is built from this address and nothing else (never from
                // the request's Host header, which a visitor controls).
                let Some(public) = &public_url else {
                    return Err(ConfigError::new(
                        "PUBLIC_URL",
                        "is required when SMTP_URL is set, because emailed links are built from it",
                    ));
                };
                let secure = public.starts_with("https://")
                    || host_of(public).is_some_and(|host| is_loopback(&host));
                if !secure {
                    return Err(ConfigError::new(
                        "PUBLIC_URL",
                        "must start with https:// when email is enabled (http:// is only \
                         allowed for localhost), since reset links are emailed",
                    ));
                }
                Some(EmailConfig { smtp_url, from })
            }
        };

        let mut trusted_proxies = Vec::new();
        if let Some(list) = get("TRUSTED_PROXIES") {
            for item in list
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                if !is_address_or_network(item) {
                    return Err(ConfigError::new(
                        "TRUSTED_PROXIES",
                        format!("{item:?} is not an IP address or a network like 10.0.0.0/8"),
                    ));
                }
                trusted_proxies.push(item.to_owned());
            }
        }

        let trusted_proxy_hops = match get("TRUSTED_PROXY_HOPS") {
            None => 0,
            Some(value) => match value.trim().parse::<usize>() {
                Ok(hops) if hops <= MAX_PROXY_HOPS => hops,
                _ => {
                    return Err(ConfigError::new(
                        "TRUSTED_PROXY_HOPS",
                        format!(
                            "expected a whole number from 0 to {MAX_PROXY_HOPS}, got {value:?}"
                        ),
                    ));
                }
            },
        };

        Ok(Self {
            database_url,
            migrate_on_start,
            registration,
            public_url,
            email,
            trusted_proxies,
            trusted_proxy_hops,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_vars(&|name| map.get(name).cloned())
    }

    fn error(vars: &[(&str, &str)]) -> String {
        config(vars).unwrap_err().to_string()
    }

    #[test]
    fn defaults_suit_local_development() {
        assert_eq!(
            config(&[]).unwrap(),
            Config {
                database_url: DEFAULT_DATABASE_URL.to_owned(),
                migrate_on_start: true,
                registration: Registration::Open,
                public_url: None,
                email: None,
                trusted_proxies: vec![],
                trusted_proxy_hops: 0,
            }
        );
    }

    #[test]
    fn empty_values_count_as_unset() {
        let c = config(&[
            ("DATABASE_URL", ""),
            ("MIGRATE_ON_START", "  "),
            ("TRUSTED_PROXIES", ""),
            ("TRUSTED_PROXY_HOPS", ""),
        ])
        .unwrap();
        assert_eq!(c, config(&[]).unwrap());
    }

    #[test]
    fn a_database_url_is_taken_as_given_and_only_sql_server_is_accepted() {
        let url = "sqlserver://sa:secret@db:1433/todoapp";
        assert_eq!(
            config(&[("DATABASE_URL", &format!(" {url} "))])
                .unwrap()
                .database_url,
            url
        );
        for bad in [
            "postgres://u:p@h/db",
            "mysql://h/db",
            "sqlite:/data/app.db",
            "/data/app.db",
            "data.db",
            "SQLSERVER://sa:secret@db/todoapp",
            "sqlserver://db/todoapp",
        ] {
            assert!(
                error(&[("DATABASE_URL", bad)]).starts_with("DATABASE_URL:"),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_error_does_not_echo_a_database_url_that_may_hold_a_password() {
        for bad in [
            "postgres://user:hunter2@host/db",
            "sqlserver://user:hunter2@host:notaport/db",
            "sqlserver://user:hunter2@host",
        ] {
            let message = error(&[("DATABASE_URL", bad)]);
            assert!(!message.contains("hunter2"), "{message}");
        }
    }

    #[test]
    fn flags_accept_common_spellings_and_reject_typos() {
        for on in ["1", "true", "TRUE", "on", "yes", " Yes "] {
            assert!(
                config(&[("MIGRATE_ON_START", on)])
                    .unwrap()
                    .migrate_on_start,
                "{on}"
            );
        }
        for off in ["0", "false", "off", "no", "NO"] {
            assert!(
                !config(&[("MIGRATE_ON_START", off)])
                    .unwrap()
                    .migrate_on_start,
                "{off}"
            );
        }
        assert!(error(&[("MIGRATE_ON_START", "flase")]).starts_with("MIGRATE_ON_START:"));
        assert!(error(&[("MIGRATE_ON_START", "2")]).contains("expected true or false"));
    }

    #[test]
    fn registration_accepts_its_three_modes_and_rejects_typos() {
        for (value, expected) in [
            ("open", Registration::Open),
            ("closed", Registration::Closed),
            ("first-user", Registration::FirstUser),
            (" CLOSED ", Registration::Closed),
            ("First-User", Registration::FirstUser),
        ] {
            assert_eq!(
                config(&[("REGISTRATION", value)]).unwrap().registration,
                expected,
                "{value}"
            );
        }
        // A typo must not silently leave sign-ups open.
        for bad in [
            "colsed",
            "close",
            "none",
            "false",
            "first_user",
            "firstuser",
            "0",
        ] {
            assert!(
                error(&[("REGISTRATION", bad)]).starts_with("REGISTRATION:"),
                "{bad}"
            );
        }
        assert_eq!(
            config(&[]).unwrap().registration,
            Registration::Open,
            "the default is unchanged"
        );
    }

    #[test]
    fn the_public_url_is_a_plain_site_address() {
        for (value, expected) in [
            ("https://todo.example.com", "https://todo.example.com"),
            ("https://todo.example.com/", "https://todo.example.com"),
            ("  http://127.0.0.1:3000  ", "http://127.0.0.1:3000"),
            ("https://[::1]:8443", "https://[::1]:8443"),
        ] {
            assert_eq!(
                config(&[("PUBLIC_URL", value)])
                    .unwrap()
                    .public_url
                    .as_deref(),
                Some(expected),
                "{value:?}"
            );
        }
        assert_eq!(config(&[]).unwrap().public_url, None);
        assert_eq!(config(&[("PUBLIC_URL", " ")]).unwrap().public_url, None);
    }

    #[test]
    fn a_public_url_that_could_mislead_or_inject_is_refused() {
        for bad in [
            "todo.example.com",
            "ftp://todo.example.com",
            "https://todo.example.com/app",
            "https://todo.example.com/?x=1",
            "https://todo.example.com#frag",
            "https://user:pw@todo.example.com",
            "https://todo example.com",
            "javascript:alert(1)",
            "https://evil.com\r\nHost: x",
            "https://",
        ] {
            assert!(
                error(&[("PUBLIC_URL", bad)]).starts_with("PUBLIC_URL:"),
                "{bad:?}"
            );
        }
    }

    const SITE: (&str, &str) = ("PUBLIC_URL", "https://todo.example.com");
    const FROM: (&str, &str) = ("MAIL_FROM", "Todo <noreply@example.com>");

    #[test]
    fn email_is_off_unless_configured() {
        assert_eq!(config(&[SITE]).unwrap().email, None);
    }

    #[test]
    fn email_accepts_encrypted_connections() {
        for url in [
            "smtps://user:p%40ss@smtp.example.com:465",
            "SMTPS://smtp.example.com",
            "smtp://user:pw@smtp.example.com:587?tls=required",
            "smtp://smtp.example.com:587?x=1&tls=required",
        ] {
            let c = config(&[SITE, FROM, ("SMTP_URL", url)]).unwrap();
            let email = c.email.unwrap();
            assert_eq!(email.smtp_url, url);
            assert_eq!(email.from, "Todo <noreply@example.com>");
        }
    }

    #[test]
    fn plain_smtp_is_allowed_only_for_this_machine() {
        for url in [
            "smtp://localhost:1025",
            "smtp://127.0.0.1:1025",
            "smtp://[::1]:1025",
            "smtp://user:pw@LocalHost:25",
        ] {
            assert!(config(&[SITE, FROM, ("SMTP_URL", url)]).is_ok(), "{url}");
        }
        for url in [
            "smtp://smtp.example.com:25",
            "smtp://smtp.example.com:587?tls=opportunistic",
            "smtp://smtp.example.com:587?tls=required2",
            "smtp://smtp.example.com:587?notls=required",
            // A look-alike: the host is example.com, not localhost.
            "smtp://localhost@smtp.example.com:25",
            "smtp://localhost.example.com:25",
            "smtp://127.0.0.1.example.com:25",
        ] {
            assert!(
                error(&[SITE, FROM, ("SMTP_URL", url)]).starts_with("SMTP_URL:"),
                "{url}"
            );
        }
    }

    #[test]
    fn nonsense_smtp_urls_are_refused_without_quoting_the_password() {
        for url in [
            "https://user:hunter2@smtp.example.com",
            "smtps://",
            "smtps://user:hunter2@",
            "smtps://user:hunter2@smtp.example.com\r\nRCPT TO:x",
            "smtps://smtp example.com",
            "hunter2",
        ] {
            let message = error(&[SITE, FROM, ("SMTP_URL", url)]);
            assert!(message.starts_with("SMTP_URL:"), "{url:?}: {message}");
            assert!(!message.contains("hunter2"), "{message}");
        }
    }

    #[test]
    fn email_needs_a_sender_and_a_public_address() {
        let url = ("SMTP_URL", "smtps://smtp.example.com");
        assert!(error(&[SITE, url]).starts_with("MAIL_FROM:"));
        assert!(error(&[FROM, url]).starts_with("PUBLIC_URL:"));
        // A sender without a mail server is a half-finished setup, not "email off".
        assert!(error(&[SITE, FROM]).starts_with("MAIL_FROM:"));
        for bad in ["not an address", "a@", "@example.com", "Todo <>"] {
            assert!(
                error(&[SITE, url, ("MAIL_FROM", bad)]).starts_with("MAIL_FROM:"),
                "{bad}"
            );
        }
    }

    #[test]
    fn emailed_links_must_be_https_except_on_this_machine() {
        let url = ("SMTP_URL", "smtps://smtp.example.com");
        assert!(
            error(&[("PUBLIC_URL", "http://todo.example.com"), FROM, url])
                .starts_with("PUBLIC_URL:")
        );
        for local in [
            "http://localhost:3000",
            "http://127.0.0.1:3000",
            "http://[::1]:3000",
        ] {
            assert!(
                config(&[("PUBLIC_URL", local), FROM, url]).is_ok(),
                "{local}"
            );
        }
        // Without email, an http address is still fine (the command line tools only print it).
        assert!(config(&[("PUBLIC_URL", "http://todo.example.com")]).is_ok());
    }

    #[test]
    fn the_smtp_password_never_appears_in_debug_output() {
        let c = config(&[
            SITE,
            FROM,
            ("SMTP_URL", "smtps://user:hunter2@smtp.example.com"),
        ])
        .unwrap();
        let shown = format!("{c:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("noreply@example.com"));
    }

    #[test]
    fn trusted_proxies_accept_addresses_and_networks() {
        let c = config(&[("TRUSTED_PROXIES", "127.0.0.1, 10.0.0.0/8 ,fd00::/8,,::1")]).unwrap();
        assert_eq!(
            c.trusted_proxies,
            ["127.0.0.1", "10.0.0.0/8", "fd00::/8", "::1"]
        );
    }

    #[test]
    fn bad_proxy_entries_are_refused_before_topcoat_could_panic_on_them() {
        for bad in [
            "not-an-ip",
            "10.0.0.0/33",
            "fd00::/129",
            "10.0.0.0/",
            "10.0.0.0/-1",
            "10.0.0.0/+8",
            "10.0.0.0/8/8",
            "999.1.1.1",
            "10.0.0.1, oops",
            "*",
        ] {
            assert!(
                error(&[("TRUSTED_PROXIES", bad)]).starts_with("TRUSTED_PROXIES:"),
                "{bad}"
            );
        }
    }

    #[test]
    fn proxy_hops_are_a_small_whole_number() {
        assert_eq!(
            config(&[("TRUSTED_PROXY_HOPS", "1")])
                .unwrap()
                .trusted_proxy_hops,
            1
        );
        assert_eq!(
            config(&[("TRUSTED_PROXY_HOPS", " 0 ")])
                .unwrap()
                .trusted_proxy_hops,
            0
        );
        assert_eq!(
            config(&[("TRUSTED_PROXY_HOPS", "8")])
                .unwrap()
                .trusted_proxy_hops,
            MAX_PROXY_HOPS
        );
        for bad in ["9", "-1", "one", "1.5", "99999999999999999999"] {
            assert!(
                error(&[("TRUSTED_PROXY_HOPS", bad)]).starts_with("TRUSTED_PROXY_HOPS:"),
                "{bad}"
            );
        }
    }
}
