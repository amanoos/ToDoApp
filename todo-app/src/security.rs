//! Security headers for every response, including errors such as 403 and 404.
//!
//! Registered as a layer without a path, so it wraps everything the router answers. A
//! normal response gets the headers directly; an error is built after it leaves the
//! layers, so for errors the headers are queued with `response_headers` instead.
//!
//! ## About the Content-Security-Policy
//!
//! Topcoat's browser runtime builds the handlers for `$(...)` closures with
//! `new Function(...)` from server-generated source (see `compile.ts` in the topcoat
//! repository), so the policy has to allow `'unsafe-eval'` for scripts. Everything else
//! is locked down: no inline scripts, no third-party origins, no plugins, no framing, no
//! `<base>` tag, and forms can only post back to this site. That limits what injected
//! markup could do, but `'unsafe-eval'` means the policy is a second line of defence,
//! not a substitute for escaping output (which Topcoat's `view!` does).
//!
//! ## Known gaps
//!
//! - Topcoat checks the `Origin` of state-changing requests *before* any layer runs, so
//!   the plain-text 403 it answers a cross-site POST with carries none of these headers.
//!   It has no HTML to attack, so the risk is negligible; set the same headers at the
//!   reverse proxy as well (which production should do anyway) to cover it.
//! - Under `topcoat dev`, the live-reload indicator asks jsDelivr for its own font and
//!   the policy blocks that. It is cosmetic and exists only in development.

use topcoat::{
    context::Cx,
    router::{
        Body, HeaderMap, HeaderName, HeaderValue, Layer, LayerFuture, Next, Path, header,
        response::response_headers,
    },
};

/// How the headers are chosen. Built once at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// Origin (`scheme://host:port`) of the `topcoat dev` server, which serves a
    /// live-reload script and a WebSocket. Only set while developing.
    dev_origin: Option<String>,
    /// Send `Strict-Transport-Security`. Off by default; see [`Policy::from_vars`].
    hsts: bool,
    /// Send the policy as `Content-Security-Policy-Report-Only`, so violations are
    /// logged by the browser but nothing is blocked. For trying a change safely.
    csp_report_only: bool,
}

/// Turns a URL such as `http://127.0.0.1:3000/x` into its origin, or `None` if it
/// isn't a plain `http(s)://host[:port]` made only of safe characters. The origin is
/// pasted into a header, so anything that could end a directive or a header value is
/// refused.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split('/').next()?;
    let safe = !authority.is_empty()
        && authority
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '[' | ']'));
    safe.then(|| format!("{scheme}://{authority}"))
}

/// The WebSocket form of an `http(s)` origin.
fn websocket_of(origin: &str) -> String {
    match origin.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}"),
        None => format!("ws://{}", origin.trim_start_matches("http://")),
    }
}

/// Whether an environment variable value means "yes".
fn is_on(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "on" | "yes"))
}

impl Policy {
    /// Reads the settings from the environment:
    ///
    /// - `TOPCOAT_DEV_URL`, set by `topcoat dev`, lets the live-reload script run.
    /// - `ENABLE_HSTS=1` sends `Strict-Transport-Security`. Turn it on only when the site
    ///   is reached over HTTPS (browsers ignore the header on plain HTTP, and a proxy
    ///   that terminates TLS may already send it).
    /// - `CSP_REPORT_ONLY=1` reports policy violations without blocking anything.
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self::from_vars(
            var("TOPCOAT_DEV_URL").as_deref(),
            var("ENABLE_HSTS").as_deref(),
            var("CSP_REPORT_ONLY").as_deref(),
        )
    }

    /// Like [`Policy::from_env`], from the values of the three variables.
    pub fn from_vars(
        dev_url: Option<&str>,
        enable_hsts: Option<&str>,
        csp_report_only: Option<&str>,
    ) -> Self {
        Self {
            dev_origin: dev_url.and_then(origin_of),
            hsts: is_on(enable_hsts),
            csp_report_only: is_on(csp_report_only),
        }
    }

    /// The Content-Security-Policy value.
    fn csp(&self) -> String {
        let (dev_script, dev_connect) = match &self.dev_origin {
            Some(origin) => (format!(" {origin}"), format!(" {}", websocket_of(origin))),
            None => (String::new(), String::new()),
        };
        [
            "default-src 'self'".to_owned(),
            format!("script-src 'self' 'unsafe-eval'{dev_script}"),
            "style-src 'self'".to_owned(),
            "img-src 'self' data:".to_owned(),
            "font-src 'self'".to_owned(),
            format!("connect-src 'self'{dev_connect}"),
            "object-src 'none'".to_owned(),
            "base-uri 'none'".to_owned(),
            "form-action 'self'".to_owned(),
            "frame-ancestors 'none'".to_owned(),
        ]
        .join("; ")
    }

    /// Every header this policy sends, except `Cache-Control` (see [`Policy::apply_to`]).
    fn headers(&self) -> Vec<(HeaderName, HeaderValue)> {
        let csp_name = if self.csp_report_only {
            "content-security-policy-report-only"
        } else {
            "content-security-policy"
        };
        let mut pairs: Vec<(&'static str, String)> = vec![
            (csp_name, self.csp()),
            ("x-content-type-options", "nosniff".to_owned()),
            // Older browsers don't know `frame-ancestors`.
            ("x-frame-options", "DENY".to_owned()),
            ("referrer-policy", "same-origin".to_owned()),
            (
                "permissions-policy",
                "camera=(), microphone=(), geolocation=(), payment=(), usb=(), \
                 serial=(), bluetooth=(), accelerometer=(), gyroscope=(), magnetometer=()"
                    .to_owned(),
            ),
            ("cross-origin-opener-policy", "same-origin".to_owned()),
            ("cross-origin-resource-policy", "same-origin".to_owned()),
            ("x-permitted-cross-domain-policies", "none".to_owned()),
        ];
        if self.hsts {
            pairs.push(("strict-transport-security", "max-age=31536000".to_owned()));
        }
        pairs
            .into_iter()
            .map(|(name, value)| {
                (
                    HeaderName::from_static(name),
                    HeaderValue::from_str(&value).expect("policy values are plain ASCII"),
                )
            })
            .collect()
    }

    /// Sets the security headers on a response that already exists. Also adds
    /// `Cache-Control: no-store` unless the response chose its own caching, so pages and
    /// form results are never kept, while hashed assets keep their long-lived caching.
    pub fn apply_to(&self, headers: &mut HeaderMap) {
        for (name, value) in self.headers() {
            headers.insert(name, value);
        }
        if !headers.contains_key(header::CACHE_CONTROL) {
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        }
    }

    /// The same headers as a map to queue for an error response, which never chooses its
    /// own caching.
    fn error_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        self.apply_to(&mut headers);
        headers
    }
}

/// A layer that adds the security headers to every response.
#[derive(Debug, Clone)]
pub struct SecurityHeaders {
    policy: Policy,
}

impl SecurityHeaders {
    /// A layer using the settings in the environment; see [`Policy::from_env`].
    pub fn from_env() -> Self {
        Self {
            policy: Policy::from_env(),
        }
    }
}

impl Layer for SecurityHeaders {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            match next.run(cx, body).await {
                Ok(mut response) => {
                    self.policy.apply_to(response.headers_mut());
                    Ok(response)
                }
                Err(error) => {
                    // The error becomes a response only after it leaves the layers, so
                    // queue the headers for the router to add then.
                    response_headers(cx).extend(self.policy.error_headers());
                    Err(error)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Splits a CSP into `directive -> sources`.
    fn directives(csp: &str) -> HashMap<String, Vec<String>> {
        csp.split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut words = part.split_whitespace();
                let name = words.next().unwrap().to_owned();
                (name, words.map(str::to_owned).collect())
            })
            .collect()
    }

    fn production() -> Policy {
        Policy::from_vars(None, None, None)
    }

    fn get<'a>(map: &'a HashMap<String, Vec<String>>, name: &str) -> &'a [String] {
        map.get(name)
            .unwrap_or_else(|| panic!("missing directive {name}"))
    }

    #[test]
    fn the_policy_locks_down_everything_except_what_topcoat_needs() {
        let csp = directives(&production().csp());
        assert_eq!(get(&csp, "default-src"), ["'self'"]);
        assert_eq!(get(&csp, "script-src"), ["'self'", "'unsafe-eval'"]);
        assert_eq!(get(&csp, "style-src"), ["'self'"]);
        assert_eq!(get(&csp, "connect-src"), ["'self'"]);
        assert_eq!(get(&csp, "object-src"), ["'none'"]);
        assert_eq!(get(&csp, "base-uri"), ["'none'"]);
        assert_eq!(get(&csp, "form-action"), ["'self'"]);
        assert_eq!(get(&csp, "frame-ancestors"), ["'none'"]);
    }

    #[test]
    fn scripts_are_never_allowed_inline_or_from_elsewhere() {
        let csp = production().csp();
        let scripts = directives(&csp);
        for source in get(&scripts, "script-src") {
            assert!(
                source == "'self'" || source == "'unsafe-eval'",
                "unexpected script source {source}"
            );
        }
        assert!(!csp.contains("'unsafe-inline'"));
        assert!(!csp.contains('*'));
        assert!(!csp.contains("http:") && !csp.contains("https:"));
    }

    #[test]
    fn the_dev_server_is_allowed_only_when_there_is_one() {
        let dev = Policy::from_vars(Some("http://127.0.0.1:59039"), None, None);
        let csp = directives(&dev.csp());
        assert!(get(&csp, "script-src").contains(&"http://127.0.0.1:59039".to_owned()));
        assert!(get(&csp, "connect-src").contains(&"ws://127.0.0.1:59039".to_owned()));
        // Nothing else changes.
        assert_eq!(get(&csp, "default-src"), ["'self'"]);

        assert!(!production().csp().contains("127.0.0.1"));
    }

    #[test]
    fn origins_are_extracted_and_junk_is_refused() {
        assert_eq!(
            origin_of("http://127.0.0.1:3000/x?y"),
            Some("http://127.0.0.1:3000".to_owned())
        );
        assert_eq!(
            origin_of("https://[::1]:8443"),
            Some("https://[::1]:8443".to_owned())
        );
        assert_eq!(websocket_of("http://127.0.0.1:1"), "ws://127.0.0.1:1");
        assert_eq!(websocket_of("https://example.com"), "wss://example.com");
        for bad in [
            "",
            "javascript:alert(1)",
            "ftp://example.com",
            "http://",
            "http://a b",
            "http://evil.com;script-src *",
            "http://evil.com'",
            "http://evil.com\r\nSet-Cookie: x=1",
            "//example.com",
            "example.com",
        ] {
            assert_eq!(origin_of(bad), None, "{bad:?}");
        }
        // A hostile value can't reach the policy at all.
        let policy = Policy::from_vars(Some("http://evil.com;script-src *"), None, None);
        assert_eq!(policy, production());
    }

    #[test]
    fn every_header_is_sent_once_with_a_valid_value() {
        let headers = production().error_headers();
        for name in [
            "content-security-policy",
            "x-content-type-options",
            "x-frame-options",
            "referrer-policy",
            "permissions-policy",
            "cross-origin-opener-policy",
            "cross-origin-resource-policy",
            "x-permitted-cross-domain-policies",
            "cache-control",
        ] {
            assert_eq!(headers.get_all(name).iter().count(), 1, "{name}");
        }
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["referrer-policy"], "same-origin");
        assert_eq!(headers["cache-control"], "no-store");
        assert!(headers.get("content-security-policy-report-only").is_none());
    }

    #[test]
    fn hsts_is_opt_in() {
        assert!(
            production()
                .error_headers()
                .get("strict-transport-security")
                .is_none()
        );
        for on in ["1", "true", "on", "yes", " 1 "] {
            let policy = Policy::from_vars(None, Some(on), None);
            assert_eq!(
                policy.error_headers()["strict-transport-security"],
                "max-age=31536000",
                "{on:?}"
            );
        }
        for off in ["0", "false", "", "2", "enable"] {
            let policy = Policy::from_vars(None, Some(off), None);
            assert!(
                policy
                    .error_headers()
                    .get("strict-transport-security")
                    .is_none(),
                "{off:?}"
            );
        }
    }

    #[test]
    fn report_only_mode_swaps_the_header_name_and_nothing_else() {
        let enforcing = production().error_headers();
        let reporting = Policy::from_vars(None, None, Some("1")).error_headers();
        assert!(reporting.get("content-security-policy").is_none());
        assert_eq!(
            reporting["content-security-policy-report-only"],
            enforcing["content-security-policy"]
        );
        assert_eq!(reporting.len(), enforcing.len());
    }

    #[test]
    fn a_response_without_caching_is_not_stored() {
        let mut headers = HeaderMap::new();
        production().apply_to(&mut headers);
        assert_eq!(headers["cache-control"], "no-store");
    }

    #[test]
    fn a_response_that_chose_its_own_caching_keeps_it() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
        production().apply_to(&mut headers);
        assert_eq!(
            headers["cache-control"],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(headers.get_all("cache-control").iter().count(), 1);
        assert_eq!(
            headers["x-content-type-options"], "nosniff",
            "still gets the rest"
        );
    }

    #[test]
    fn existing_security_headers_are_replaced_not_duplicated() {
        let mut headers = HeaderMap::new();
        headers.insert("x-frame-options", HeaderValue::from_static("ALLOWALL"));
        headers.insert(
            "content-security-policy",
            HeaderValue::from_static("default-src *"),
        );
        production().apply_to(&mut headers);
        assert_eq!(headers.get_all("x-frame-options").iter().count(), 1);
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers.get_all("content-security-policy").iter().count(), 1);
        assert!(
            headers["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("'self'")
        );
    }
}
