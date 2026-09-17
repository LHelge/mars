//! All SQL, through `sqlx::query!` / `query_as!`. One `XRepository<'a>` per
//! aggregate, borrowing the `PgPool`, with the scope in the `WHERE` clause.

pub mod users;

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
