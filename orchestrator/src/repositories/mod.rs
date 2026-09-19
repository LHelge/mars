//! All SQL, through `sqlx::query!` / `query_as!`. One `XRepository<'a>` per
//! aggregate, borrowing the `PgPool`, with the scope in the `WHERE` clause.

pub mod password_reset_tokens;
pub mod projects;
pub mod refresh_tokens;
pub mod secrets;
pub mod sessions;
pub mod tasks;
pub mod user_invites;
pub mod users;

pub use password_reset_tokens::PasswordResetTokenRepository;
pub use projects::ProjectRepository;
pub use refresh_tokens::RefreshTokenRepository;
pub use secrets::{SecretListFilter, SecretRepository, UserFilter};
pub use sessions::{
    AppendedRange, CostDelta, MAX_EVENT_PAGE, ProcessStart, SessionRepository, Transition,
};
pub use tasks::{StateFields, TaskFilter, TaskRepository};
pub use user_invites::UserInviteRepository;
pub use users::UserRepository;

// The crate convention (`CLAUDE.md`, "Backend conventions"); here it is what
// makes the `Error` doc link below resolve.
#[allow(unused_imports)]
use crate::prelude::*;

/// The name of the unique constraint `err` violated, if it violated one.
///
/// Every repository turns a duplicate row into [`Error::Conflict`] with a
/// message the caller can act on, and every repository has to recognise the
/// same shape of `sqlx::Error` to do it. Matching on the constraint name
/// rather than on the error string keeps that mapping tied to the migration
/// that created the index:
///
/// ```ignore
/// match unique_violation(&err) {
///     Some("users_username_key") => Error::Conflict("username already taken".into()),
///     _ => Error::from(err),
/// }
/// ```
///
/// Postgres always reports the constraint for a unique violation, but the
/// return stays an `Option` because `constraint()` is optional in the driver's
/// own interface; an unnamed violation falls through to the generic mapping,
/// which answers 500 rather than inventing a message.
pub(crate) fn unique_violation(err: &sqlx::Error) -> Option<&str> {
    match err {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            database_error.constraint()
        }
        _ => None,
    }
}

/// The name of the `CHECK` constraint `err` violated, if it violated one.
///
/// [`unique_violation`] for the table checks a caller can actually break. Most
/// of them are unreachable behind a validated model — `max_attempts` is a
/// [`crate::models::MaxAttempts`] long before it is a `SMALLINT` — but a check
/// that spans columns cannot be decided by one model: `projects_check` says a
/// `ready` project has a `default_branch`, and that is a fact about the row
/// the clone job is updating, not about its argument. Recognising it here lets
/// the repository answer 409 with a message instead of 500 with none.
///
/// An unnamed `CHECK` in a migration gets Postgres's default name,
/// `<table>_check` for a table constraint and `<table>_<column>_check` for a
/// column one, so the constant a caller matches on is still tied to the
/// migration.
pub(crate) fn check_violation(err: &sqlx::Error) -> Option<&str> {
    match err {
        sqlx::Error::Database(database_error) if database_error.is_check_violation() => {
            database_error.constraint()
        }
        _ => None,
    }
}

/// The SQLSTATE Postgres raises when an `ON DELETE RESTRICT` reference stops a
/// delete: `restrict_violation`, *not* the `foreign_key_violation` 23503 that
/// `sqlx`'s own `is_foreign_key_violation()` looks for.
const RESTRICT_VIOLATION: &str = "23001";

/// The name of the foreign key `err` violated, if it violated one.
///
/// The sibling of the two above, for the `ON DELETE RESTRICT` references:
/// `sessions_profile_id_fkey` is what makes deleting a profile that still has
/// sessions a 409 rather than an internal error (`docs/data-model.md`,
/// `agent_profiles`).
///
/// Both codes are accepted because Postgres uses two. Inserting a child whose
/// parent is missing is 23503, which `is_foreign_key_violation()` recognises;
/// deleting a parent that a `RESTRICT` child still references is
/// [`RESTRICT_VIOLATION`], which it does not. A caller that only checked the
/// first would answer 500 to the one case this crate actually has —
/// `tasks.state_id` and `sessions.profile_id` are both `RESTRICT`.
pub(crate) fn foreign_key_violation(err: &sqlx::Error) -> Option<&str> {
    match err {
        sqlx::Error::Database(database_error)
            if database_error.is_foreign_key_violation()
                || database_error.code().as_deref() == Some(RESTRICT_VIOLATION) =>
        {
            database_error.constraint()
        }
        _ => None,
    }
}

/// [`Error::Conflict`] with `message` when `err` is a unique violation of
/// `constraint`, and the generic mapping otherwise.
///
/// The one-constraint shape of the `match` in `users::map_duplicate`, for
/// the repositories that have exactly one duplicate a caller can provoke:
///
/// ```ignore
/// .map_err(|err| conflict_on(err, "user_invites_open_email_idx", "an open invite already exists for this email"))
/// ```
///
/// It takes `err` by value because the only thing left to do with a mapped
/// error is return it: a violation of some other constraint widens through
/// `#[from] sqlx::Error` here, which logs the detail and answers 500 rather
/// than inventing a message for a constraint this call site did not name.
pub(crate) fn conflict_on(err: sqlx::Error, constraint: &str, message: &str) -> Error {
    match unique_violation(&err) {
        Some(violated) if violated == constraint => Error::Conflict(message.to_string()),
        _ => Error::from(err),
    }
}
