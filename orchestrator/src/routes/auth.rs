//! `POST /api/auth/login`, `POST /api/auth/refresh` and `POST /api/auth/logout`
//! (`SPEC.md`, "Auth (`/api/auth`)").
//!
//! The three routes that hand out credentials, and the only ones that write
//! `refresh_tokens` rows for an ordinary sign-in. What they share is the rule
//! from `docs/data-model.md`, "Users and authentication" and ADR 0025:
//!
//! > Login, refresh, reset-link issuance, password changes and reset-token
//! > consumption lock the user row before locking or writing that user's token
//! > rows. Re-read and validate credentials under that lock. Expensive password
//! > hashing may happen beforehand, but an earlier password check must be
//! > revalidated against the locked row before issuing credentials. [...]
//! > Return credentials only after commit.
//!
//! So both login and refresh have the same three-part shape: do the slow or
//! unlocked work first, then take the user-row lock and *redo the decision*
//! against what the lock returned, then commit and only then mint the access
//! token and set the cookie. That is what makes a concurrent password change
//! either revoke the new credential or be observed by it, never neither.
//!
//! None of the three takes an extractor from `routes::extractors`: login
//! authenticates with a password, refresh and logout with the cookie, and a
//! user with `must_change_password` reaches all three (`SPEC.md`,
//! "Authentication").
//!
//! **Lock order.** One user-row lock, then that user's `refresh_tokens` rows.
//! No project, session or administrator-membership lock is taken here.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Router};
use axum_extra::extract::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::models::user::verify_password;
use crate::models::{OpaqueToken, RefreshToken, User};
use crate::prelude::*;
use crate::repositories::{RefreshTokenRepository, UserRepository};
use crate::routes::cookies::{clear_refresh_cookie, refresh_cookie};
use crate::routes::throttle::client_addr;

/// What a failed refresh or logout-shaped authentication answers with, the
/// same string `routes::extractors` uses (`SPEC.md`, "Authentication").
const AUTHENTICATION_REQUIRED: &str = "authentication required";

/// What login answers for an unknown username *and* for a wrong password.
///
/// One message for both: which of the two a caller got wrong is exactly what
/// an account-enumeration probe is after.
const INVALID_CREDENTIALS: &str = "invalid username or password";

/// The 429 message (`SPEC.md`, "Auth (`/api/auth`)").
const TOO_MANY_LOGIN_ATTEMPTS: &str = "too many login attempts";

/// An Argon2id hash a login for an unknown username is verified against.
///
/// Not a credential: the PHC string of the obviously fake password
/// `not-a-real-password`, generated once with
/// [`crate::models::user::hash_password`] and pasted here (`CLAUDE.md`, rule 3).
/// Nothing authenticates against it and no row carries it.
///
/// Its purpose is timing. Without it, a login for a name that exists costs one
/// Argon2 verification and a login for a name that does not costs none, and
/// the difference is measurable from outside — which would turn the
/// deliberately identical [`INVALID_CREDENTIALS`] message back into an
/// enumeration oracle. Verifying the submitted password against this hash
/// spends the same work and throws the answer away.
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$8oilaKuGRDWMSAJB+d33OQ$mq7rf+s/87LSWeNRWB94b5LtVYCVoG9hLW46mzmZ5N0";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
}

/// `POST /auth/login` (`SPEC.md`, "Auth (`/api/auth`)").
///
/// A missing or malformed field is 400 through the [`Json`] extractor; the
/// username is trimmed before anything is looked up and the password is used
/// exactly as sent.
#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

/// `{ user, access_token }`: what login, invite acceptance and a self-service
/// password change all answer with (`SPEC.md`, "Authentication").
///
/// `user` serialises through the [`User`] model, which skips `password_hash`,
/// `auth_version` and `updated_at`, so the body is the documented `User` DTO
/// and the refresh token is not in it — that one only ever travels in the
/// cookie.
///
/// `pub(crate)` because the accept-invite and password-change routes return
/// the same body and must not restate its shape.
#[derive(Debug, Serialize)]
pub(crate) struct TokenPairResponse {
    pub user: User,
    pub access_token: String,
}

/// Insert a fresh refresh token for `user` inside the caller's transaction.
///
/// Returns the raw token — the value that goes in the cookie and nowhere else
/// — together with the stored row. The caller is holding `user`'s row lock
/// (`docs/data-model.md`), has already revalidated whatever it is issuing
/// credentials on, and sends the cookie only after its transaction commits.
///
/// `pub(crate)` for the three other places that issue a pair: accept-invite, a
/// self-service password change and the test-only login fixture.
pub(crate) async fn issue_pair(
    state: &AppState,
    tx: &mut PgConnection,
    user: &User,
) -> Result<(String, RefreshToken)> {
    let token = OpaqueToken::generate();
    let expires_at = Utc::now() + REFRESH_TOKEN_TTL;

    let stored = RefreshTokenRepository::new(&state.pool)
        .insert(tx, user.id, &token.hash, expires_at)
        .await?;

    Ok((token.raw, stored))
}

/// Exchange a username and password for a token pair.
///
/// The order is the contract, not an implementation detail:
///
/// 1. the throttle, *before* any hashing, so a blocked key costs one hash-free
///    request (`SPEC.md`, "Authentication");
/// 2. the unlocked lookup and the Argon2 verification, off the runtime's
///    worker threads;
/// 3. the user-row lock, the revalidation against the locked row and the
///    insert, in one transaction;
/// 4. the access token and the cookie, from the committed row.
async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(body): Json<LoginRequest>,
) -> Result<(StatusCode, CookieJar, Json<TokenPairResponse>)> {
    let username = body.username.trim().to_string();
    let addr = client_addr(&headers, peer.map(|Extension(info)| info));

    if state.login_throttle.check(&username, addr).is_err() {
        // Deliberately no `record_failure`: a blocked request must not push
        // its own block further out (`SPEC.md`, "Authentication").
        info!(username = %username, addr = %addr, "login refused: throttled");
        return Err(Error::Throttled(TOO_MANY_LOGIN_ATTEMPTS.to_string()));
    }

    let users = UserRepository::new(&state.pool);
    let candidate = users.find_by_username(&username).await?;

    // One Argon2 verification either way; see `DUMMY_PASSWORD_HASH`.
    let hash = candidate
        .as_ref()
        .map_or(DUMMY_PASSWORD_HASH, |user| user.password_hash.as_str())
        .to_string();
    let verified = verify_blocking(hash, body.password.clone()).await?;

    let (Some(user), true) = (candidate, verified) else {
        return Err(reject_login(&state, &username, addr));
    };

    let mut tx = state.pool.begin().await?;

    // From here on the row this returns is the only authority: a password
    // change may have committed while this request waited for the lock.
    let Some(locked) = users.lock_user(&mut tx, user.id).await? else {
        // Deleted between the lookup and the lock.
        return Err(reject_login(&state, &username, addr));
    };

    // Revalidate the password against the locked row (`docs/data-model.md`;
    // ADR 0025). An unchanged hash means the verification above was about
    // exactly this row and still stands; a changed one is re-verified rather
    // than assumed wrong, because a user may have "changed" their password to
    // the same string.
    if locked.password_hash != user.password_hash
        && !verify_blocking(locked.password_hash.clone(), body.password.clone()).await?
    {
        return Err(reject_login(&state, &username, addr));
    }

    let (raw, _) = issue_pair(&state, &mut tx, &locked).await?;
    tx.commit().await?;

    // Only after the commit (`docs/data-model.md`), and from the locked row,
    // so the claims carry the committed `auth_version`, `admin` and
    // `must_change_password`.
    let access_token = Claims::for_user(&locked, Utc::now()).encode(&state.config)?;
    state.login_throttle.record_success(&username);

    Ok((
        StatusCode::OK,
        jar.add(refresh_cookie(&state.config, &raw)),
        Json(TokenPairResponse {
            user: locked,
            access_token,
        }),
    ))
}

/// Rotate the refresh cookie and mint a new pair from the current user row.
///
/// Authenticated by the cookie alone, so every rejection is the same 401 as a
/// missing one and every rejection also clears the cookie (`SPEC.md`,
/// "Authentication"): a browser holding a token the database will never accept
/// again should stop sending it.
///
/// A 5xx does *not* clear it. A refresh that failed because Postgres was
/// briefly unreachable is a retry, not a sign-out, and clearing the cookie
/// would turn one transient failure into a forced login.
///
/// This is the one handler in the crate that builds its own [`Response`]
/// rather than returning [`Result`]: the failure path has to carry a
/// `Set-Cookie` header as well as the error, and the crate-wide [`Error`] is
/// deliberately a plain enum with no room for one. All the fallible work is in
/// [`rotate`], which returns `Result` like everything else.
async fn refresh(State(state): State<AppState>, jar: CookieJar) -> Response {
    match rotate(&state, &jar).await {
        Ok((raw, user, access_token)) => (
            StatusCode::OK,
            jar.add(refresh_cookie(&state.config, &raw)),
            Json(TokenPairResponse { user, access_token }),
        )
            .into_response(),
        Err(err) if err.status() == StatusCode::UNAUTHORIZED => {
            (jar.add(clear_refresh_cookie(&state.config)), err).into_response()
        }
        Err(err) => err.into_response(),
    }
}

/// The body of [`refresh`]: everything that can fail, so the caller decides
/// once what to do with the cookie.
///
/// Returns the raw replacement token, the user as read under the lock and the
/// matching access token.
async fn rotate(state: &AppState, jar: &CookieJar) -> Result<(String, User, String)> {
    let Some(raw) = presented_token(jar) else {
        return Err(unauthorized());
    };

    let hash = OpaqueToken::hash_of(&raw);
    let tokens = RefreshTokenRepository::new(&state.pool);
    let users = UserRepository::new(&state.pool);

    // Unlocked, and only to learn which user row to lock: a request arrives
    // with a cookie and nothing else. Nothing is decided from this row —
    // `revoked_at` and `expires_at` are read again under the lock below.
    let Some(located) = tokens.find_by_hash(&hash).await? else {
        return Err(unauthorized());
    };

    let mut tx = state.pool.begin().await?;

    let Some(user) = users.lock_user(&mut tx, located.user_id).await? else {
        // A cookie for a user who has been deleted.
        return Err(unauthorized());
    };

    // The authoritative read. A password change that committed while this
    // request waited for the lock has already set `revoked_at`, so this is
    // where the revocation race is decided (ADR 0025).
    let Some(current) = tokens
        .find_by_hash_for_user(&mut tx, &hash, user.id)
        .await?
    else {
        return Err(unauthorized());
    };

    if !current.is_usable(Utc::now()) {
        debug!(user_id = %user.id, "refresh token is revoked or expired");
        return Err(unauthorized());
    }

    tokens.revoke(&mut tx, current.id).await?;
    let (replacement, _) = issue_pair(state, &mut tx, &user).await?;
    tx.commit().await?;

    // Built from the locked row, so a demotion or a raised
    // `must_change_password` is reflected in the new pair (`SPEC.md`,
    // "Authentication": "using the current user values").
    let access_token = Claims::for_user(&user, Utc::now()).encode(&state.config)?;

    Ok((replacement, user, access_token))
}

/// Revoke the presented refresh token and clear the cookie.
///
/// Always 204, with or without a cookie and whether or not the token was still
/// usable: logging out is not an operation that can be refused, and telling a
/// caller that their cookie named nothing would be an oracle over stored
/// tokens. Only the presented token is revoked — the user's other browsers
/// stay signed in — so no user-row lock is needed.
async fn logout(State(state): State<AppState>, jar: CookieJar) -> Result<(StatusCode, CookieJar)> {
    if let Some(raw) = presented_token(&jar) {
        RefreshTokenRepository::new(&state.pool)
            .revoke_by_hash(&OpaqueToken::hash_of(&raw))
            .await?;
    }

    Ok((
        StatusCode::NO_CONTENT,
        jar.add(clear_refresh_cookie(&state.config)),
    ))
}

/// The raw refresh token in `jar`, if the cookie is there and not empty.
///
/// An empty value is treated as absent: that is exactly what
/// [`clear_refresh_cookie`] sets, and a browser that has not yet dropped the
/// cleared cookie should get the same answer as one that has.
fn presented_token(jar: &CookieJar) -> Option<String> {
    jar.get(REFRESH_COOKIE)
        .map(|cookie| cookie.value().to_string())
        .filter(|raw| !raw.is_empty())
}

/// Count a failed login attempt and produce its 401.
///
/// The one place login failure is logged, at `info` with the username and the
/// address and nothing else: never the password, never the user id of a row
/// that was found, never which of the two halves was wrong (rule 3).
fn reject_login(state: &AppState, username: &str, addr: std::net::IpAddr) -> Error {
    state.login_throttle.record_failure(username, addr);
    info!(username = %username, addr = %addr, "login failed");
    Error::Unauthorized(INVALID_CREDENTIALS.to_string())
}

/// The 401 every cookie-authenticated rejection shares.
fn unauthorized() -> Error {
    Error::Unauthorized(AUTHENTICATION_REQUIRED.to_string())
}

/// Verify `candidate` against `hash` on the blocking pool.
///
/// Argon2id at the OWASP parameters is tens of milliseconds of CPU, which is
/// the point of it; running that on a runtime worker would stall every other
/// task on that thread for the duration, so a handful of concurrent logins
/// would stall the whole orchestrator.
///
/// Both arguments are owned because they cross a thread boundary. Neither is
/// ever logged, and the `JoinError` of a panicking task widens into the
/// crate-wide 500 rather than into a failed verification: a check that did not
/// run must not authenticate anyone.
async fn verify_blocking(hash: String, candidate: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || verify_password(&hash, &candidate))
        .await
        .map_err(|err| {
            error!(error = %err, "the password verification task did not finish");
            Error::Internal("password verification failed".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The password [`DUMMY_PASSWORD_HASH`] was generated from. Obviously
    /// fake, and used only to prove the constant is a real Argon2id PHC
    /// string: a truncated or misquoted one would silently fail to parse, the
    /// verification would return in microseconds and the timing defence would
    /// be gone without any test noticing.
    const DUMMY_PASSWORD: &str = "not-a-real-password";

    #[test]
    fn the_dummy_hash_is_a_working_argon2id_hash() {
        assert!(verify_password(DUMMY_PASSWORD_HASH, DUMMY_PASSWORD));
        assert!(!verify_password(DUMMY_PASSWORD_HASH, "something-else"));
    }

    #[test]
    fn no_row_can_ever_carry_the_dummy_hash() {
        // It is a constant in this file and nothing writes it, but the salt is
        // what makes that structural: hashing the same password again gives a
        // different PHC string, so no user who happened to choose
        // `not-a-real-password` would end up matching it.
        let other = crate::models::user::hash_password(DUMMY_PASSWORD).expect("hashing succeeds");
        assert_ne!(other, DUMMY_PASSWORD_HASH);
    }

    #[tokio::test]
    async fn verification_on_the_blocking_pool_agrees_with_the_model() {
        assert!(
            verify_blocking(DUMMY_PASSWORD_HASH.to_string(), DUMMY_PASSWORD.to_string())
                .await
                .expect("the task finishes")
        );
        assert!(
            !verify_blocking(DUMMY_PASSWORD_HASH.to_string(), "wrong".to_string())
                .await
                .expect("the task finishes")
        );
        // A hash this build cannot parse is a failed verification, not an
        // error the login handler has to distinguish.
        assert!(
            !verify_blocking(
                "$argon2id$fake$hash".to_string(),
                DUMMY_PASSWORD.to_string()
            )
            .await
            .expect("the task finishes")
        );
    }
}
