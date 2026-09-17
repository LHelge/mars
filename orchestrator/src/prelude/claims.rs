//! The access-token claims and their signing.
//!
//! `SPEC.md`, "Authentication" is the contract: the access token is a JWT
//! signed with `JWT_SECRET` carrying `sub`, `auth_version`, `username`,
//! `admin`, `must_change_password`, `iat` and `exp`, and it is short lived
//! ([`ACCESS_TOKEN_TTL`]).
//!
//! The claims are a *snapshot*, never an authority. `admin` and
//! `must_change_password` are here so the frontend can render without a second
//! round trip; authorization always reads the current row (ADR 0025). What the
//! token does decide is identity ([`Claims::sub`]) and the revocation
//! generation ([`Claims::auth_version`]), and both are checked against the
//! database on every request.
//!
//! This module signs and verifies only. It touches neither the database nor
//! `AppState`, so the HTTP, WebSocket, SSE and MCP layers can all reach for it.

use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, errors::ErrorKind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::User;
use crate::prelude::*;

/// Why an access token was not accepted.
///
/// The two variants exist for the log line and for tests; both render the same
/// message and the same status, because telling a caller *why* their token
/// failed tells an attacker which half of a forgery worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimsError {
    /// Malformed, wrongly signed, signed with another algorithm, or carrying
    /// claims that are not the documented shape.
    #[error("authentication required")]
    Invalid,
    /// Well formed and correctly signed, but `exp` has passed.
    #[error("authentication required")]
    Expired,
}

impl ClaimsError {
    /// The HTTP status this rejection maps to.
    ///
    /// Always 401: an access token that does not verify leaves the request
    /// unauthenticated, whatever went wrong with it (`SPEC.md`,
    /// "Authentication").
    pub fn status(&self) -> axum::http::StatusCode {
        axum::http::StatusCode::UNAUTHORIZED
    }
}

/// The access token's payload (`SPEC.md`, "Authentication").
///
/// `iat` and `exp` are Unix seconds, as JWT requires. The field names are the
/// claim names, so this struct *is* the wire format; renaming a field changes
/// the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// The user id. Decoding fails unless it parses as a UUID.
    pub sub: Uuid,
    /// The revocation generation this token was minted for; a request is
    /// rejected unless it still equals `users.auth_version` (ADR 0025).
    pub auth_version: i64,
    /// A snapshot, for display only.
    pub username: String,
    /// A snapshot; authorization re-reads the current row.
    pub admin: bool,
    /// A snapshot; the password-change gate re-reads the current row.
    pub must_change_password: bool,
    /// Issued at, Unix seconds.
    pub iat: i64,
    /// Expires at, Unix seconds. Strictly enforced, with no leeway.
    pub exp: i64,
}

impl Claims {
    /// The claims for `user` as of `now`, expiring [`ACCESS_TOKEN_TTL`] later.
    pub fn for_user(user: &User, now: DateTime<Utc>) -> Self {
        let iat = now.timestamp();
        Self {
            sub: user.id,
            auth_version: user.auth_version,
            username: user.username.clone(),
            admin: user.admin,
            must_change_password: user.must_change_password,
            iat,
            exp: iat + ACCESS_TOKEN_TTL.num_seconds(),
        }
    }

    /// Sign these claims with `JWT_SECRET` as an HS256 token.
    pub fn encode(&self, config: &Config) -> std::result::Result<String, ClaimsError> {
        let key = EncodingKey::from_secret(config.jwt_secret.as_bytes());
        jsonwebtoken::encode(&Header::new(Algorithm::HS256), self, &key).map_err(|err| {
            // Signing cannot fail on caller input, so this is a configuration
            // or crypto-provider fault worth a log line. The error never
            // carries the secret.
            error!(error = %err, "signing an access token failed");
            ClaimsError::Invalid
        })
    }

    /// Verify `token` against `JWT_SECRET` and return its claims.
    ///
    /// The signature, the `HS256` header and `exp` are all checked, with no
    /// leeway: `jsonwebtoken` defaults to sixty seconds of slack, which would
    /// quietly extend every access token, so it is set to zero here. `iat` is
    /// not checked against the clock; a token from a slightly fast issuer is
    /// still this issuer's token.
    ///
    /// Restricting the accepted algorithms to HS256 is what makes an `alg:
    /// none` or an asymmetric-to-symmetric confusion token fail.
    pub fn decode(token: &str, config: &Config) -> std::result::Result<Self, ClaimsError> {
        let key = DecodingKey::from_secret(config.jwt_secret.as_bytes());

        let mut validation = Validation::new(Algorithm::HS256);
        validation.algorithms = vec![Algorithm::HS256];
        validation.leeway = 0;
        validation.validate_exp = true;
        validation.set_required_spec_claims(&["exp", "sub"]);

        match jsonwebtoken::decode::<Self>(token, &key, &validation) {
            Ok(data) => Ok(data.claims),
            Err(err) if matches!(err.kind(), ErrorKind::ExpiredSignature) => {
                Err(ClaimsError::Expired)
            }
            Err(_) => Err(ClaimsError::Invalid),
        }
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use chrono::TimeDelta;
    use std::collections::HashMap;

    use super::*;

    /// Obviously fake, and the only signing secret any test here uses
    /// (`CLAUDE.md`, rule 3).
    const TEST_SECRET: &str = "not-a-real-signing-secret";

    /// Not a credential: a syntactically valid PHC string with a fake body.
    const FAKE_HASH: &str = "$argon2id$fake$hash";

    fn config() -> Config {
        let vars: HashMap<&str, &str> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", TEST_SECRET),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "unix:///run/user/1000/podman/podman.sock"),
            ("DATA_DIR_HOST", "/srv/mars/data"),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
            ("SESSION_IMAGE_DEFAULT", "mars-session-claude:dev"),
        ]
        .into_iter()
        .collect();

        Config::from_vars(|name| vars.get(name).map(|value| value.to_string()))
            .expect("the test configuration loads")
    }

    fn user() -> User {
        User {
            id: Uuid::new_v4(),
            username: "ada".into(),
            email: "ada@example.com".into(),
            password_hash: FAKE_HASH.into(),
            auth_version: 7,
            must_change_password: true,
            admin: true,
            notify_email: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn claims_for_a_user_expire_fifteen_minutes_later() {
        let user = user();
        let now = Utc::now();

        let claims = Claims::for_user(&user, now);

        assert_eq!(claims.sub, user.id);
        assert_eq!(claims.auth_version, 7);
        assert_eq!(claims.username, "ada");
        assert!(claims.admin);
        assert!(claims.must_change_password);
        assert_eq!(claims.iat, now.timestamp());
        assert_eq!(claims.exp, now.timestamp() + 900);
    }

    #[test]
    fn a_token_round_trips() {
        let config = config();
        let claims = Claims::for_user(&user(), Utc::now());

        let token = claims.encode(&config).unwrap();
        let decoded = Claims::decode(&token, &config).unwrap();

        assert_eq!(decoded, claims);
    }

    #[test]
    fn a_token_never_carries_the_signing_secret() {
        let token = Claims::for_user(&user(), Utc::now())
            .encode(&config())
            .unwrap();
        assert!(!token.contains(TEST_SECRET), "leaked: {token}");
    }

    #[test]
    fn an_expired_token_is_rejected_as_expired() {
        let config = config();
        let long_ago = Utc::now() - TimeDelta::hours(2);
        let token = Claims::for_user(&user(), long_ago).encode(&config).unwrap();

        assert_eq!(Claims::decode(&token, &config), Err(ClaimsError::Expired));
    }

    #[test]
    fn expiry_has_no_leeway() {
        let config = config();
        // One second past `exp`: the jsonwebtoken default of sixty seconds of
        // leeway would accept this.
        let issued = Utc::now() - ACCESS_TOKEN_TTL - TimeDelta::seconds(1);
        let token = Claims::for_user(&user(), issued).encode(&config).unwrap();

        assert_eq!(Claims::decode(&token, &config), Err(ClaimsError::Expired));
    }

    #[test]
    fn a_tampered_signature_is_rejected() {
        let config = config();
        let token = Claims::for_user(&user(), Utc::now())
            .encode(&config)
            .unwrap();

        let (body, signature) = token.rsplit_once('.').unwrap();
        // Flip one character of the signature, keeping it valid base64url.
        let mut signature = signature.to_string();
        let last = signature.pop().unwrap();
        signature.push(if last == 'A' { 'B' } else { 'A' });
        let tampered = format!("{body}.{signature}");

        assert_eq!(
            Claims::decode(&tampered, &config),
            Err(ClaimsError::Invalid)
        );
    }

    #[test]
    fn a_token_signed_with_another_secret_is_rejected() {
        let mut other = config();
        other.jwt_secret = "a-different-fake-secret".into();
        let token = Claims::for_user(&user(), Utc::now())
            .encode(&other)
            .unwrap();

        assert_eq!(Claims::decode(&token, &config()), Err(ClaimsError::Invalid));
    }

    #[test]
    fn an_unsigned_token_is_rejected() {
        let config = config();
        let claims = Claims::for_user(&user(), Utc::now());

        // `alg: none` with an empty signature: the classic downgrade.
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let token = format!("{header}.{payload}.");

        assert_eq!(Claims::decode(&token, &config), Err(ClaimsError::Invalid));
    }

    #[test]
    fn a_subject_that_is_not_a_uuid_is_rejected() {
        let config = config();
        let claims = serde_json::json!({
            "sub": "not-a-uuid",
            "auth_version": 0,
            "username": "ada",
            "admin": false,
            "must_change_password": false,
            "iat": Utc::now().timestamp(),
            "exp": (Utc::now() + ACCESS_TOKEN_TTL).timestamp(),
        });
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
        )
        .unwrap();

        assert_eq!(Claims::decode(&token, &config), Err(ClaimsError::Invalid));
    }

    #[test]
    fn garbage_is_rejected() {
        let config = config();
        for token in ["", "not-a-token", "a.b.c", "....."] {
            assert_eq!(Claims::decode(token, &config), Err(ClaimsError::Invalid));
        }
    }

    #[test]
    fn both_rejections_are_401_and_say_nothing_more() {
        for error in [ClaimsError::Invalid, ClaimsError::Expired] {
            assert_eq!(error.status(), axum::http::StatusCode::UNAUTHORIZED);
            assert_eq!(error.to_string(), "authentication required");
        }
    }
}
