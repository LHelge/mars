//! The bearer middleware every MCP request passes through
//! (`ARCHITECTURE.md`, "MCP design" → "Authentication").
//!
//! One layer, one contract: hash the presented token, find the session it
//! belongs to, refuse a session that has ended and attach the resolved
//! [`SessionContext`] to the request. The handlers behind it never take a
//! session id, so a caller can only ever act as the session whose token it
//! holds (`ARCHITECTURE.md`, "Trust boundaries", item 2).
//!
//! The lookup is one query per HTTP request and nothing is cached. That is
//! deliberate: a relaunch replaces `sessions.mcp_token_hash` (ADR 0029), and
//! with no cache the old token stops working on its very next request, whatever
//! `Mcp-Session-Id` it belongs to. A session that ends mid-conversation is
//! refused on its next call for the same reason.
//!
//! Rule 3 of `CLAUDE.md` governs every line here: the raw token is hashed and
//! dropped, and neither it nor its hash is ever put in a log field, a span or a
//! response body. A refusal says only what a caller has to fix.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::models::SessionState;
use crate::prelude::*;
use crate::repositories::{ProjectRepository, SessionRepository};
use crate::session::token::hash_mcp_token;

use super::SessionContext;

/// What a caller that presented no usable token is told. The same message for a
/// missing header, a wrong scheme and an unknown token, so the endpoint does not
/// answer the question "is this token one of yours?".
const UNAUTHORIZED: &str = "missing or invalid bearer token";

/// What a caller whose session is `done` or `failed` is told.
const ENDED: &str = "session has ended";

/// Authenticate one MCP request and attach its [`SessionContext`].
///
/// Installed on the `/mcp` service with
/// [`axum::middleware::from_fn_with_state`] in [`super::mcp_router`], so the
/// router's 404 fallback stays unauthenticated — a container pointed at the
/// wrong path gets `not found` rather than `unauthorized`.
///
/// The order of the checks is the documented one:
///
/// 1. no `Authorization` header, a scheme other than `Bearer`, or an empty
///    token → 401,
/// 2. a hash that matches no session → 401 with the same message,
/// 3. a session that is `done` or `failed` → 403,
/// 4. otherwise the request proceeds with the context in its extensions.
///
/// Every method goes through it: the `POST` that carries JSON-RPC, the `GET`
/// that opens the SSE stream and the `DELETE` that closes an MCP session alike.
pub async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(&request) else {
        return Error::Unauthorized(UNAUTHORIZED.into()).into_response();
    };

    let context = match resolve(&state, token).await {
        Ok(context) => context,
        Err(err) => return err.into_response(),
    };

    debug!(session_id = %context.session_id, "mcp request authenticated");
    request.extensions_mut().insert(context);

    next.run(request).await
}

/// The token in `Authorization: Bearer <token>`, or `None` if the header is
/// absent, is not a `Bearer` credential or carries nothing.
///
/// The header is read with `HeaderMap::get`, which takes the first of a
/// repeated header and ignores the rest. The scheme is compared without regard
/// to case, as RFC 7235 requires; the credential is not touched at all, so a
/// token with surrounding whitespace is a token that does not match a stored
/// hash — except for the space that separates it from the scheme, which is the
/// delimiter and not part of the value.
fn bearer_token(request: &Request) -> Option<&str> {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;

    let (scheme, credential) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") || credential.is_empty() {
        return None;
    }

    Some(credential)
}

/// The session this token belongs to, with its profile.
///
/// `token` is consumed here and hashed immediately; nothing below this line
/// holds the raw value.
async fn resolve(state: &AppState, token: &str) -> Result<SessionContext> {
    let hash = hash_mcp_token(token);

    let Some(row) = SessionRepository::new(&state.pool)
        .find_by_mcp_token_hash(&hash)
        .await?
    else {
        return Err(Error::Unauthorized(UNAUTHORIZED.into()));
    };

    if matches!(row.state, SessionState::Done | SessionState::Failed) {
        return Err(Error::Forbidden(ENDED.into()));
    }

    // `sessions.profile_id` is `ON DELETE RESTRICT`, so a live session always
    // has its profile; `None` here is a schema fault, not a caller's mistake.
    let Some(profile) = ProjectRepository::new(&state.pool)
        .find_profile(row.project_id, row.profile_id)
        .await?
    else {
        error!(session_id = %row.session_id, "mcp session has no profile");
        return Err(Error::Internal("session profile is missing".into()));
    };

    Ok(SessionContext {
        session_id: row.session_id,
        project_id: row.project_id,
        profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    fn request(authorization: Option<&str>) -> Request {
        let mut builder = HttpRequest::builder().uri("/mcp");
        if let Some(value) = authorization {
            builder = builder.header(axum::http::header::AUTHORIZATION, value);
        }
        builder.body(Body::empty()).expect("a request builds")
    }

    #[test]
    fn a_bearer_credential_is_taken_as_it_stands() {
        assert_eq!(bearer_token(&request(Some("Bearer abc"))), Some("abc"));
        // The scheme is case-insensitive; the credential is not touched.
        assert_eq!(bearer_token(&request(Some("bearer aBc"))), Some("aBc"));
    }

    #[test]
    fn anything_that_is_not_a_bearer_credential_is_nothing() {
        assert_eq!(bearer_token(&request(None)), None);
        assert_eq!(bearer_token(&request(Some("Basic xyz"))), None);
        assert_eq!(bearer_token(&request(Some("Bearer"))), None);
        assert_eq!(bearer_token(&request(Some("Bearer "))), None);
        assert_eq!(bearer_token(&request(Some("abc"))), None);
    }

    #[test]
    fn a_padded_credential_keeps_its_padding_and_so_matches_nothing() {
        // Not rejected here — it is simply not the token that was stored, and
        // the hash lookup is what says so.
        assert_eq!(bearer_token(&request(Some("Bearer  abc"))), Some(" abc"));
    }

    #[test]
    fn the_first_authorization_header_wins() {
        let mut builder = HttpRequest::builder().uri("/mcp");
        builder = builder.header(axum::http::header::AUTHORIZATION, "Bearer first");
        builder = builder.header(axum::http::header::AUTHORIZATION, "Bearer second");
        let request = builder.body(Body::empty()).expect("a request builds");

        assert_eq!(bearer_token(&request), Some("first"));
    }

    #[test]
    fn the_refusals_are_the_documented_statuses_and_messages() {
        let unauthorized = Error::Unauthorized(UNAUTHORIZED.into());
        assert_eq!(unauthorized.status().as_u16(), 401);
        assert_eq!(
            unauthorized.user_message(),
            "missing or invalid bearer token"
        );

        let forbidden = Error::Forbidden(ENDED.into());
        assert_eq!(forbidden.status().as_u16(), 403);
        assert_eq!(forbidden.user_message(), "session has ended");
    }
}
