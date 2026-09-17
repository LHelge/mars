//! The refresh-token cookie, built in one place.
//!
//! `SPEC.md`, "Authentication" fixes the attributes: `HttpOnly`,
//! `SameSite=Lax`, `Path=/` and `Secure` when `PUBLIC_URL` is https. Login,
//! accept-invite, refresh, a self-service password change and logout all set or
//! clear the same cookie, and a browser only replaces a cookie when the name,
//! domain and path match — so a second copy of these attributes somewhere else
//! would not overwrite the first one, it would sit beside it. Hence: two
//! functions, no attributes written anywhere else.
//!
//! Pure; neither function reads the database or `AppState`.

use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::TimeDelta;

use crate::prelude::*;

/// The cookie carrying `raw`, valid for [`REFRESH_TOKEN_TTL`].
///
/// `raw` is the opaque token itself (`models::OpaqueToken`), not its stored
/// hash. It reaches the browser here and nowhere else.
pub fn refresh_cookie(config: &Config, raw: &str) -> Cookie<'static> {
    build(config, raw.to_string(), REFRESH_TOKEN_TTL)
}

/// The same cookie, empty and already expired, which is how a browser is told
/// to drop it.
///
/// Every attribute except the value and `Max-Age` has to match
/// [`refresh_cookie`] or the browser keeps the original (`SPEC.md`,
/// "Authentication": logout and a failed refresh both clear the cookie).
pub fn clear_refresh_cookie(config: &Config) -> Cookie<'static> {
    build(config, String::new(), TimeDelta::zero())
}

/// The one place the attributes are written.
fn build(config: &Config, value: String, max_age: TimeDelta) -> Cookie<'static> {
    let mut cookie = Cookie::new(REFRESH_COOKIE, value);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie.set_secure(is_https(&config.public_url));
    // `Max-Age` is a `time::Duration`, a type the `cookie` crate re-exports but
    // `axum-extra` does not, so it is reached by conversion rather than by
    // name; that keeps `time` out of this crate's dependencies
    // (`ARCHITECTURE.md`, "Orchestrator internals": one crate per concern).
    // The TTLs here are small positive constants, so the conversion cannot
    // fail, and `None` would only mean a session cookie.
    let max_age = std::time::Duration::from_secs(max_age.num_seconds().unsigned_abs());
    cookie.set_max_age(max_age.try_into().ok());
    cookie
}

/// Whether `public_url` is an https URL.
///
/// [`Config`] already rejects a `PUBLIC_URL` without an `http://` or `https://`
/// prefix, but the comparison is case-insensitive here anyway: `Secure` is the
/// flag that keeps the refresh token off a plaintext connection, and it must
/// not turn on a spelling.
fn is_https(public_url: &str) -> bool {
    public_url
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Obviously fake; no test here needs a real token (`CLAUDE.md`, rule 3).
    const FAKE_RAW: &str =
        "00000000-0000-4000-8000-000000000001.00000000-0000-4000-8000-000000000002";

    fn config(public_url: &str) -> Config {
        let vars: HashMap<&str, String> = [
            ("PUBLIC_URL", public_url.to_string()),
            ("JWT_SECRET", "not-a-real-signing-secret".to_string()),
            (
                "DATABASE_URL",
                "postgres://mars:fake@localhost:5432/mars".to_string(),
            ),
            (
                "DOCKER_HOST",
                "unix:///run/user/1000/podman/podman.sock".to_string(),
            ),
            ("DATA_DIR_HOST", "/srv/mars/data".to_string()),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key".to_string()),
            ("GIT_BOT_NAME", "Mars Bot".to_string()),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid".to_string()),
            (
                "SESSION_IMAGE_DEFAULT",
                "mars-session-claude:dev".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        Config::from_vars(|name| vars.get(name).cloned()).expect("the test configuration loads")
    }

    /// The `Set-Cookie` value a response would carry.
    fn rendered(cookie: &Cookie<'static>) -> String {
        cookie.to_string()
    }

    fn assert_shared_attributes(cookie: &Cookie<'static>) {
        assert_eq!(cookie.name(), "refresh_token");
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.path(), Some("/"));
    }

    #[test]
    fn the_cookie_carries_the_documented_attributes() {
        let cookie = refresh_cookie(&config("https://mars.example.invalid"), FAKE_RAW);

        assert_shared_attributes(&cookie);
        assert_eq!(cookie.value(), FAKE_RAW);

        let rendered = rendered(&cookie);
        assert!(rendered.contains("HttpOnly"), "{rendered}");
        assert!(rendered.contains("SameSite=Lax"), "{rendered}");
        assert!(rendered.contains("Path=/"), "{rendered}");
        assert!(rendered.contains("Max-Age=2592000"), "{rendered}");
    }

    #[test]
    fn max_age_is_thirty_days() {
        let cookie = refresh_cookie(&config("https://mars.example.invalid"), FAKE_RAW);

        assert_eq!(
            cookie.max_age().expect("a max-age").whole_seconds(),
            30 * 24 * 60 * 60
        );
        assert_eq!(REFRESH_TOKEN_TTL.num_seconds(), 30 * 24 * 60 * 60);
    }

    #[test]
    fn https_makes_the_cookie_secure() {
        let cookie = refresh_cookie(&config("https://mars.example.invalid"), FAKE_RAW);

        assert_eq!(cookie.secure(), Some(true));
        assert!(
            rendered(&cookie).contains("Secure"),
            "{}",
            rendered(&cookie)
        );
    }

    #[test]
    fn plain_http_does_not() {
        // Local development over http: a `Secure` cookie would never be sent
        // back and login would appear to succeed and then forget itself.
        let cookie = refresh_cookie(&config("http://localhost:8080"), FAKE_RAW);

        assert_eq!(cookie.secure(), Some(false));
        assert!(
            !rendered(&cookie).contains("Secure"),
            "{}",
            rendered(&cookie)
        );
    }

    #[test]
    fn a_trailing_slash_or_a_path_does_not_change_the_secure_flag() {
        // `Config` strips the trailing slash; the flag is decided by the
        // scheme either way.
        assert!(
            refresh_cookie(&config("https://mars.example.invalid/"), FAKE_RAW)
                .secure()
                .unwrap()
        );
        assert!(
            !refresh_cookie(&config("http://localhost:8080/mars"), FAKE_RAW)
                .secure()
                .unwrap()
        );
    }

    #[test]
    fn the_scheme_is_compared_case_insensitively() {
        assert!(is_https("HTTPS://mars.example.invalid"));
        assert!(is_https("HttpS://mars.example.invalid"));
        assert!(!is_https("http://localhost:8080"));
        assert!(!is_https("HTTP://localhost:8080"));
        // Shorter than the prefix, and multi-byte, neither of which may panic.
        assert!(!is_https(""));
        assert!(!is_https("https:/"));
        assert!(!is_https("h†tps://x"));
    }

    #[test]
    fn clearing_keeps_every_attribute_but_empties_the_value() {
        for public_url in ["https://mars.example.invalid", "http://localhost:8080"] {
            let config = config(public_url);
            let set = refresh_cookie(&config, FAKE_RAW);
            let cleared = clear_refresh_cookie(&config);

            assert_shared_attributes(&cleared);
            assert_eq!(cleared.value(), "");
            assert_eq!(cleared.secure(), set.secure());
            assert_eq!(cleared.max_age().expect("a max-age").whole_seconds(), 0);

            let rendered = rendered(&cleared);
            assert!(rendered.contains("Max-Age=0"), "{rendered}");
            assert!(!rendered.contains(FAKE_RAW), "leaked: {rendered}");
        }
    }
}
