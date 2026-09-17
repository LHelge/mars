//! Items every module imports with `use crate::prelude::*;`.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" lists what lives here:
//! [`AppState`], [`Config`], [`Claims`], [`Error`] and the crate-wide
//! [`Result`] alias, together with the handful of third-party names —
//! [`Arc`], [`PgPool`] and the `tracing` macros — that appear in nearly every
//! module and would otherwise be re-imported by hand each time.
//!
//! The credential lifetimes below are here for the same reason: the routes, the
//! repositories and the cron cleanup job all have to agree on them, and an
//! expiry that is written twice is an expiry that will disagree once.
//!
//! Nothing that is specific to one concern belongs here: traits, models and
//! repositories stay in their own modules and are imported explicitly.

use chrono::TimeDelta;

pub mod claims;
pub mod config;
pub mod error;
pub mod state;
pub mod telemetry;

pub use claims::{Claims, ClaimsError};
pub use config::{Config, ConfigError, SecretsMasterKeySource};
pub use error::{Error, Json, Result};
pub use state::AppState;
pub use telemetry::{TelemetryError, init_tracing};

pub use sqlx::PgPool;
pub use std::sync::Arc;
pub use tracing::{debug, error, info, instrument, warn};

/// How long an access token is valid (`SPEC.md`, "Authentication").
///
/// Short on purpose: it is the window in which a password change, a demotion
/// or a deletion is not yet reflected in the token's own claims. It is not the
/// window in which they take effect — every request re-reads the user row, so
/// the claims are only a snapshot (ADR 0025).
///
/// `TimeDelta` is `chrono`'s duration, which these are `const` in; it is also
/// signed and arithmetic-compatible with the `DateTime<Utc>` values the
/// repositories store, which `std::time::Duration` is not.
pub const ACCESS_TOKEN_TTL: TimeDelta = TimeDelta::minutes(15);

/// How long a refresh token is valid (`docs/data-model.md`, `refresh_tokens`).
///
/// Also the `Max-Age` of the `refresh_token` cookie, so a browser stops sending
/// a token the database would reject anyway.
pub const REFRESH_TOKEN_TTL: TimeDelta = TimeDelta::days(30);

/// How long an invitation link is valid (`docs/data-model.md`,
/// `user_invites`). Resending an invite starts a new one.
pub const INVITE_TTL: TimeDelta = TimeDelta::days(7);

/// How long a password-reset link is valid (`SPEC.md`, "User-facing
/// features"; `docs/data-model.md`, `password_reset_tokens`).
///
/// An hour rather than the invitation's seven days: a reset link is asked for
/// by somebody who is at the keyboard now, and it is the one credential a
/// stranger can cause to be mailed to an address they do not control. Asking
/// again costs one request, and `routes::throttle::RESET_WINDOW` — the hour
/// three of them are allowed in — is deliberately the same length.
pub const PASSWORD_RESET_TTL: TimeDelta = TimeDelta::hours(1);

/// The name of the refresh-token cookie (`SPEC.md`, "Authentication").
///
/// A browser matches a cookie by name, domain and path, so this spelling is
/// part of the wire contract: changing it signs everyone out.
pub const REFRESH_COOKIE: &str = "refresh_token";
