//! The authentication the WebSocket and SSE session streams share
//! (`SPEC.md`, "Authentication"; ADR 0025).
//!
//! A browser cannot put an `Authorization` header on a `WebSocket` or an
//! `EventSource`, so both endpoints take the access token as `?token=` and
//! "validate it exactly like the header". That is two different moments, and
//! this module is both of them:
//!
//! | Moment | Here | Checks |
//! | --- | --- | --- |
//! | the stream opens | [`StreamToken`] + [`authenticate_stream`] | signature, expiry, the user row, `auth_version`, the password-change gate |
//! | before every incoming application message, and at each ping / keepalive tick | [`reauthorize`] | the user row, `auth_version`, the password-change gate |
//!
//! The second column is deliberately shorter: "Token signature and expiry are
//! checked when a stream opens; an open stream is not closed merely because
//! that token later expires." So [`reauthorize`] never decodes the JWT again
//! — it only re-reads the state the opening check read from the database,
//! through the [`StreamPrincipal`] captured at open.
//!
//! The rule both handlers exist to get right is the failure mode: a database
//! error is [`StreamAuthFailure::Unavailable`], which is *not* a pass.
//! "Database failures must not authorize input or continued streaming; close
//! and let normal retry handle them." Both variants of
//! [`StreamAuthFailure`] mean close the stream.
//!
//! Nothing here takes the user-row mutation lock: "Ordinary request
//! authorization reads current user state without taking this mutation lock"
//! (`docs/data-model.md`, "Users and authentication"). A re-authorization that
//! races a password change therefore passes until the change commits and fails
//! afterwards, which is the documented one-tick lag ("Revocation detection on
//! idle streams can lag by one heartbeat interval").

use std::fmt;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::User;
use crate::prelude::*;
use crate::repositories::UserRepository;
use crate::routes::extractors::{authenticate_access_token, bearer_token};

/// The message a stream that failed to authenticate answers with, in the 401
/// body and in the WebSocket `error` message alike, so the two cannot drift
/// apart.
///
/// The same string as [`AUTHENTICATION_REQUIRED`], which is where it is
/// defined; this is the name the stream handlers spell it under.
pub const AUTH_REQUIRED: &str = AUTHENTICATION_REQUIRED;

/// The access token a stream endpoint was opened with.
///
/// Read from `?token=`, falling back to `Authorization: Bearer` when the query
/// string carries none — browsers cannot send the header, but the integration
/// tests and any non-browser client can, and accepting both costs nothing. The
/// query parameter wins when both are present.
#[derive(Clone)]
pub struct StreamToken(pub String);

/// Hand-written so a token never reaches a log through a `{:?}` of something
/// that happens to contain one (rule 3).
impl fmt::Debug for StreamToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamToken(<redacted>)")
    }
}

/// Only `token`; `after` and anything else a stream endpoint takes is parsed
/// by the handler itself, which is why this deserializes from the same query
/// string without `deny_unknown_fields`.
#[derive(Debug, Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: Option<String>,
}

impl FromRequestParts<AppState> for StreamToken {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let Query(query) = Query::<TokenQuery>::from_request_parts(parts, state).await?;

        // A `?token=` that is there but empty is treated as absent rather than
        // as a separate failure: either way there is no token in the query
        // string, and the header is the documented second place to look.
        let from_query = query
            .token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string);

        match from_query {
            Some(token) => Ok(Self(token)),
            None => Ok(Self(bearer_token(&parts.headers)?)),
        }
    }
}

/// The user behind a stream's opening token, or the failure that closes it.
///
/// Exactly the [`CurrentUser`](crate::routes::CurrentUser) contract, spelled
/// without headers: [`authenticate_access_token`] for the signature, the
/// expiry, the user row and `auth_version` — all 401 — and then the
/// password-change gate, 403 `password change required`. The same token
/// therefore gets the same answer here as it would on any ordinary route.
pub async fn authenticate_stream(state: &AppState, token: &str) -> Result<User> {
    let user = authenticate_access_token(state, token).await?;

    if user.must_change_password {
        debug!(user_id = %user.id, "stream refused by the password-change gate");
        return Err(Error::Forbidden(PASSWORD_CHANGE_REQUIRED.to_string()));
    }

    Ok(user)
}

/// Who a stream is open as, captured once at open.
///
/// The two things [`reauthorize`] compares, and nothing else: the token is not
/// kept, because re-checking its expiry is exactly what the specification says
/// not to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPrincipal {
    pub user_id: Uuid,
    pub auth_version: i64,
}

impl StreamPrincipal {
    /// The principal for the user [`authenticate_stream`] returned.
    pub fn new(user: &User) -> Self {
        Self {
            user_id: user.id,
            auth_version: user.auth_version,
        }
    }
}

/// Why a stream may no longer continue. Both variants mean: close it.
///
/// The distinction is for the log line and for the message the client is sent,
/// never for the decision — a handler that treats [`Unavailable`] as a pass
/// has reintroduced the bug this module exists to prevent.
///
/// [`Unavailable`]: StreamAuthFailure::Unavailable
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StreamAuthFailure {
    /// The account is gone, was re-authenticated elsewhere, or now has to
    /// change its password.
    #[error("{AUTH_REQUIRED}")]
    Revoked,
    /// The check could not be made. Not an authorization.
    #[error("{AUTH_REQUIRED}")]
    Unavailable,
}

/// Re-run the per-message and per-tick half of the stream contract.
///
/// One unlocked read of the `users` row (`docs/data-model.md`): the row still
/// exists, its `auth_version` is still the one the stream opened with, and the
/// password-change gate is not set. No JWT is decoded — see the module
/// documentation.
///
/// The result is deliberately not the crate's [`Result`]: a caller has to
/// handle [`StreamAuthFailure::Unavailable`], and widening a database error
/// into a 500 here would let it be logged and dropped instead.
pub async fn reauthorize(
    state: &AppState,
    principal: &StreamPrincipal,
) -> std::result::Result<(), StreamAuthFailure> {
    let user = UserRepository::new(&state.pool)
        .find(principal.user_id)
        .await
        .map_err(|error| {
            // The id only; never the token (rule 3).
            error!(user_id = %principal.user_id, %error, "stream re-authorization could not read the user row");
            StreamAuthFailure::Unavailable
        })?;

    let Some(user) = user else {
        debug!(user_id = %principal.user_id, "stream closed: the user no longer exists");
        return Err(StreamAuthFailure::Revoked);
    };

    if user.auth_version != principal.auth_version {
        debug!(user_id = %principal.user_id, "stream closed: a superseded login generation");
        return Err(StreamAuthFailure::Revoked);
    }

    if user.must_change_password {
        debug!(user_id = %principal.user_id, "stream closed by the password-change gate");
        return Err(StreamAuthFailure::Revoked);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_token_never_prints_itself() {
        let token = StreamToken("not-a-real-token".to_string());

        assert_eq!(format!("{token:?}"), "StreamToken(<redacted>)");
        assert!(!format!("{token:?}").contains("not-a-real-token"));
    }

    #[test]
    fn both_failures_are_the_one_authentication_message() {
        assert_eq!(StreamAuthFailure::Revoked.to_string(), AUTH_REQUIRED);
        assert_eq!(StreamAuthFailure::Unavailable.to_string(), AUTH_REQUIRED);
        assert_eq!(AUTH_REQUIRED, "authentication required");
    }
}
