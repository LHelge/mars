//! Login credentials: issuing, rotating and revoking them, and the cookie they
//! travel in (`ARCHITECTURE.md`, "User authentication and revocation";
//! `SPEC.md`, "Authentication").
//!
//! Everything that hands a browser a way to authenticate is here.
//! [`Credentials`] is the only thing in the crate that mints an access token or
//! builds a `refresh_token` cookie, and the only thing that issues an invite or
//! reset link. The route layer above it parses a request, calls one method and
//! shapes the answer; it never opens a transaction, never locks a user row and
//! never spells the cookie's name.
//!
//! That concentration is the point. `docs/data-model.md`, "Users and
//! authentication" and ADR 0025 require the same five-step shape of every
//! credential mutation — do the slow work unlocked, lock the user row, redo the
//! decision against the row the lock returned, commit, and only then issue —
//! and a shape repeated in seven handlers is a shape six of them will get right.
//! Written once, no route can get it wrong.
//!
//! The read side deliberately stays where it is:
//! [`crate::routes::extractors`] and `authenticate_access_token` verify a token
//! that already exists, which is a different job from producing one.

pub mod cookies;
pub mod credentials;

pub use cookies::REFRESH_COOKIE;
pub use credentials::{Credentials, IssuedPair};

/// A link into the frontend, for an emailed invitation or reset.
///
/// `Config` already strips a trailing slash from `public_url`, but the join
/// trims one anyway: the two callers are the only places a raw token becomes a
/// URL, and a `//` in an emailed link is the kind of thing that is noticed
/// after it has been sent. The raw token is never logged here — only
/// `LogEmailClient` writes a whole link down, and only when `RESEND_API_KEY` is
/// unset (rule 3, ADR 0026).
pub(crate) fn public_link(
    config: &crate::prelude::Config,
    segment: &str,
    raw_token: &str,
) -> String {
    format!(
        "{}/{}/{}",
        config.public_url.trim_end_matches('/'),
        segment,
        raw_token
    )
}
