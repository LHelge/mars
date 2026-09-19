//! The extractors every authenticated route is written in terms of.
//!
//! `SPEC.md`, "Authentication" is the contract: an access token is validated
//! for signature and expiry, the current user row is loaded, the claim's
//! `auth_version` has to still equal `users.auth_version`, and authorization
//! reads the database's `admin` and `must_change_password` rather than the
//! token's snapshots (ADR 0025). A missing user or a version mismatch is 401,
//! so deletion and a password change take effect on the next request without
//! any token blacklist; a demotion takes effect the same way, as a 403.
//!
//! Three extractors, because the gate has exactly two documented exceptions:
//!
//! | Extractor | Used by |
//! | --- | --- |
//! | [`UngatedUser`] | `GET /users/me` and `POST /users/{id}/password`, the two routes a user with `must_change_password` still reaches |
//! | [`CurrentUser`] | every other authenticated handler |
//! | [`AdminUser`] | the administrator-only handlers |
//!
//! `POST /auth/refresh` and `POST /auth/logout` take none of them: they
//! authenticate with the refresh cookie, not with a bearer token.
//!
//! [`authenticate_access_token`] is the part that does not touch headers, so
//! the WebSocket and SSE endpoints — which cannot receive an `Authorization`
//! header from a browser and take `?token=` instead — validate a token
//! identically, and re-run the same checks at their heartbeat ticks.
//!
//! Nothing here takes the user-row mutation lock: "Ordinary request
//! authorization reads current user state without taking this mutation lock"
//! (`docs/data-model.md`, "Users and authentication").
//!
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::models::User;
use crate::prelude::*;
use crate::repositories::UserRepository;

/// The current user behind `token`, or 401.
///
/// In order (`SPEC.md`, "Authentication"): verify the signature and the
/// expiry, load the user the claims name, require the claim's `auth_version`
/// to still match the row. Every rejection is 401 with
/// [`AUTHENTICATION_REQUIRED`]; a database failure widens into the crate-wide
/// 500 instead, because a lookup that did not answer must never authorize.
///
/// Header-free on purpose: the `?token=` streams call this directly
/// (`SPEC.md`, "Authentication": they "validate it exactly like the header").
pub async fn authenticate_access_token(state: &AppState, token: &str) -> Result<User> {
    let claims = Claims::decode(token, &state.config)?;

    let Some(user) = UserRepository::new(&state.pool).find(claims.sub).await? else {
        // The id only; never the token, and never the rest of the claims.
        debug!(user_id = %claims.sub, "access token names a user that no longer exists");
        return Err(Error::Unauthorized(AUTHENTICATION_REQUIRED.to_string()));
    };

    if claims.auth_version != user.auth_version {
        debug!(user_id = %claims.sub, "access token is from a superseded login generation");
        return Err(Error::Unauthorized(AUTHENTICATION_REQUIRED.to_string()));
    }

    Ok(user)
}

/// An authenticated user, with the password-change gate *not* applied.
///
/// Only for the two routes `SPEC.md`, "Authentication" exempts: `GET
/// /users/me`, so the frontend can render the signed-in user it is about to
/// send to the change-password page, and `POST /users/{id}/password`, which is
/// how the gate is cleared. Anything else takes [`CurrentUser`].
#[derive(Debug, Clone)]
pub struct UngatedUser(pub User);

impl FromRequestParts<AppState> for UngatedUser {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let token = bearer_token(&parts.headers)?;
        Ok(Self(authenticate_access_token(state, &token).await?))
    }
}

/// An authenticated user who is not waiting to change their password.
///
/// What nearly every handler extracts. Adds the gate to [`UngatedUser`]: while
/// the *current row* says `must_change_password`, every endpoint outside the
/// documented exceptions answers 403 `password change required`.
#[derive(Debug, Clone)]
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let UngatedUser(user) = UngatedUser::from_request_parts(parts, state).await?;

        if user.must_change_password {
            debug!(user_id = %user.id, "request refused by the password-change gate");
            return Err(Error::Forbidden(PASSWORD_CHANGE_REQUIRED.to_string()));
        }

        Ok(Self(user))
    }
}

/// An authenticated administrator.
///
/// The role is read from the database row, never from the token's `admin`
/// claim, which is a display snapshot: a demotion therefore takes effect on
/// the demoted administrator's next request without revoking anything (ADR
/// 0025).
///
/// Built on [`CurrentUser`], so the password-change gate runs *first*: an
/// administrator who has to change their password is told `password change
/// required`, not `admin required`.
#[derive(Debug, Clone)]
pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;

        if !user.admin {
            debug!(user_id = %user.id, "request refused by an administrator-only route");
            return Err(Error::Forbidden(ADMIN_REQUIRED.to_string()));
        }

        Ok(Self(user))
    }
}

/// The token out of `Authorization: Bearer <token>`, or 401.
///
/// The scheme is compared case-insensitively, as RFC 9110 requires; a header
/// that is missing, not ASCII, carries another scheme or carries an empty
/// token is the same 401 as a token that does not verify. The token itself is
/// never logged (rule 3).
fn bearer_token(headers: &HeaderMap) -> Result<String> {
    let unauthorized = || Error::Unauthorized(AUTHENTICATION_REQUIRED.to_string());

    let value = headers
        .get(AUTHORIZATION)
        .ok_or_else(unauthorized)?
        .to_str()
        .map_err(|_| unauthorized())?;

    let (scheme, token) = value.split_once(' ').ok_or_else(unauthorized)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(unauthorized());
    }

    let token = token.trim();
    if token.is_empty() {
        return Err(unauthorized());
    }

    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderValue, StatusCode};

    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn a_bearer_token_is_read_whatever_the_scheme_s_case() {
        for value in [
            "Bearer abc.def.ghi",
            "bearer abc.def.ghi",
            "BEARER abc.def.ghi",
        ] {
            assert_eq!(bearer_token(&headers(value)).unwrap(), "abc.def.ghi");
        }
    }

    #[test]
    fn surrounding_space_is_not_part_of_the_token() {
        assert_eq!(bearer_token(&headers("Bearer   abc  ")).unwrap(), "abc");
    }

    #[test]
    fn a_missing_malformed_or_foreign_header_is_401() {
        let mut cases = vec![HeaderMap::new()];
        for value in ["", "Bearer", "Bearer ", "Basic abc", "Bearerabc", " abc"] {
            cases.push(headers(value));
        }

        for headers in cases {
            let error = bearer_token(&headers).expect_err("the header must be rejected");
            assert_eq!(error.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(error.to_string(), AUTHENTICATION_REQUIRED);
        }
    }

    #[test]
    fn a_non_ascii_header_is_401_rather_than_a_panic() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_bytes(b"Bearer \xff\xfe").expect("an opaque header value"),
        );

        assert!(matches!(
            bearer_token(&headers),
            Err(Error::Unauthorized(_))
        ));
    }
}
