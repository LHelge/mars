//! Users and the three authentication tables that hang off them.
//!
//! `docs/data-model.md`, "Users and authentication" is the column contract;
//! `SPEC.md`, "Users" is the field contract the API exposes and `SPEC.md`,
//! "User-facing features" is where the 10–128 character password rule comes
//! from. Validation lives here, SQL lives in `repositories/` (`CLAUDE.md`,
//! "Backend conventions").
//!
//! What this module guarantees is the *stored* form of the three fields a
//! unique index or a login depends on — the username, the email and the
//! password the caller chose — so a row can never reach the database untrimmed,
//! differently cased or out of bounds. It also owns the one-way function that
//! turns a [`Password`] into the `password_hash` column and the one that checks
//! a candidate against it ([`hash_password`], [`verify_password`]); issuing
//! credentials from that check is the routes' job, not this module's.

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions"). Models report
// their own error rather than the crate-wide one, so the glob is here for the
// doc links and for what these models grow into.
#[allow(unused_imports)]
use crate::prelude::*;

/// Shortest accepted username, in characters (`docs/data-model.md`, `users`).
pub const MIN_USERNAME_CHARS: usize = 3;

/// Longest accepted username, in characters (`docs/data-model.md`, `users`).
pub const MAX_USERNAME_CHARS: usize = 32;

/// Shortest accepted password, in characters (`SPEC.md`, "User-facing
/// features").
pub const MIN_PASSWORD_CHARS: usize = 10;

/// Longest accepted password, in characters (`SPEC.md`, "User-facing
/// features"). The bound is on the plaintext the user typed; what reaches the
/// database is an Argon2id PHC string of fixed length.
pub const MAX_PASSWORD_CHARS: usize = 128;

/// Every way a user model can reject its input.
///
/// `Display` is the message the API returns in `{ status, error }`, so each
/// variant says what the caller has to change and never carries internal
/// detail — in particular it never echoes the rejected value, which for
/// [`UserError::InvalidPassword`] would be the password itself (`CLAUDE.md`,
/// rule 3). [`UserError::status`] is the HTTP status the crate-wide [`Error`]
/// delegates to (`ARCHITECTURE.md`, "Orchestrator internals").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UserError {
    /// The username was not 3–32 characters after trimming.
    #[error("username must be 3-32 characters")]
    InvalidUsername,
    /// The password was not 10–128 characters.
    #[error("password must be 10-128 characters")]
    InvalidPassword,
    /// The email was not a single `local@domain` address.
    #[error("email must be an address of the form local@domain")]
    InvalidEmail,
    /// Argon2 could not hash the password. Not the caller's fault: with the
    /// default parameters and a generated salt this only happens if the
    /// system's randomness or the hasher itself fails.
    #[error("hashing the password failed")]
    Hash,
}

impl UserError {
    /// The HTTP status this rejection maps to.
    ///
    /// Every input rejection is 400: a username or email that is well formed
    /// but already taken is decided against the database and surfaces as
    /// [`Error::Conflict`] from the repository instead (`SPEC.md`, "Users").
    /// [`UserError::Hash`] is the exception — it is this process failing, not
    /// the caller — so it is 500 and the crate-wide [`Error`] logs it and
    /// answers with the generic message.
    pub fn status(&self) -> StatusCode {
        match self {
            UserError::InvalidUsername | UserError::InvalidPassword | UserError::InvalidEmail => {
                StatusCode::BAD_REQUEST
            }
            UserError::Hash => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// The Argon2id context new hashes are derived with.
///
/// `Argon2::default()` is Argon2id v19 at the OWASP-recommended parameters
/// (`m=19456, t=2, p=1`), the same ones the seeded administrator's hash in the
/// `users` migration was produced with.
#[cfg(not(feature = "integration-tests"))]
fn hasher() -> Argon2<'static> {
    Argon2::default()
}

/// The cheapest parameters the algorithm accepts — 8 KiB, one pass, one lane —
/// in a build that carries the `integration-tests` feature.
///
/// Nothing in such a build is real: it is the test harness and the end-to-end
/// orchestrator, whose users, passwords and databases exist for the length of
/// one run (`CLAUDE.md`, "Testing expectations"). The default parameters are
/// tens of milliseconds of CPU *per hash* by design, and a suite that creates
/// a handful of users per test spends more time deriving keys than testing.
///
/// Only derivation is affected. [`verify_password`] takes its parameters from
/// the stored hash, so a production hash still verifies at production cost in
/// a test build, and a test hash is recognisable by its `m=8` — nothing
/// written by this build can be mistaken for a production hash.
#[cfg(feature = "integration-tests")]
fn hasher() -> Argon2<'static> {
    use argon2::{Algorithm, Params, Version};

    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(Params::MIN_M_COST, 1, 1, None).expect("the test parameters are in range"),
    )
}

/// The Argon2id PHC string to store in `users.password_hash`.
///
/// [`hasher`] generates a fresh 16-byte salt per call — so hashing one
/// password twice gives two different strings, which is the point of a salt
/// (`docs/data-model.md`, `users`).
///
/// This is CPU-bound for tens of milliseconds by design. Request handlers call
/// it inside `tokio::task::spawn_blocking` so one login cannot stall the
/// runtime's worker thread.
pub fn hash_password(password: &str) -> UserResult<String> {
    hasher()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|err| {
            // The error describes parameters and lengths, never the password.
            error!(error = %err, "hashing a password failed");
            UserError::Hash
        })
}

/// Whether `candidate` is the password behind `hash`.
///
/// Total: a `hash` that is not a PHC string this build understands — truncated,
/// from another algorithm, or empty because some row was written wrong — is a
/// failed verification, not a panic and not an error the caller has to handle.
/// The comparison itself is constant-time inside `argon2`.
///
/// The parameters come from `hash`, not from [`hash_password`], so hashes
/// written with older cost settings keep verifying after the defaults move.
///
/// As CPU-bound as [`hash_password`], and called the same way.
pub fn verify_password(hash: &str, candidate: &str) -> bool {
    Argon2::default()
        .verify_password(candidate.as_bytes(), hash)
        .is_ok()
}

/// The result type the user models return.
///
/// The prelude's `Result` is the crate-wide one; models report their own error
/// and let `?` widen it at the call site.
pub type UserResult<T> = std::result::Result<T, UserError>;

/// A validated username: trimmed, 3–32 characters.
///
/// Case is preserved, so `users_username_key` treats `Ada` and `ada` as two
/// different names. Only the email is normalised (see [`Email`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Username(String);

impl Username {
    /// Trim `raw` and accept it when 3–32 characters remain.
    pub fn parse(raw: &str) -> UserResult<Self> {
        let trimmed = raw.trim();
        let length = trimmed.chars().count();
        if !(MIN_USERNAME_CHARS..=MAX_USERNAME_CHARS).contains(&length) {
            return Err(UserError::InvalidUsername);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The trimmed username, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Username> for String {
    fn from(username: Username) -> Self {
        username.0
    }
}

impl std::fmt::Display for Username {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated plaintext password: 10–128 characters, exactly as typed.
///
/// Whitespace is significant in a password, so unlike [`Username`] and
/// [`Email`] this type never trims or normalises: the bytes that were typed
/// are the bytes that get hashed. It is a short-lived value on its way to
/// Argon2 and is never stored, serialised or logged — `Debug` prints a
/// placeholder so a password cannot reach a log line through a `?` field or a
/// derived `Debug` on some struct that happens to hold one (`CLAUDE.md`, rule
/// 3). There is deliberately no `Display` and no `Serialize`; reading the
/// plaintext takes the explicit [`Password::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    /// Accept `raw` when it is 10–128 characters.
    pub fn parse(raw: &str) -> UserResult<Self> {
        let length = raw.chars().count();
        if !(MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&length) {
            return Err(UserError::InvalidPassword);
        }
        Ok(Self(raw.to_string()))
    }

    /// The plaintext, for hashing or verification only.
    ///
    /// Named so that every call site reads as a deliberate exposure; the
    /// authentication epic is the only caller.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

/// A validated email address: trimmed, lower-cased, one `local@domain`.
///
/// The normalisation is the point: `users_email_key` and
/// `user_invites_open_email_idx` compare the stored bytes, so ` Ada@Example.COM `
/// and `ada@example.com` have to collide (`docs/data-model.md`, "Users and
/// authentication"). The length limit is the other half: the column is `TEXT`,
/// so [`EMAIL_MAX_CHARS`] is what keeps an address the mail provider could
/// never accept out of an invite. Anything beyond the shape and the length
/// below is left to delivery — addresses arrive from an invite that was
/// actually sent, so a syntactic check here only has to keep the unique index
/// honest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Email(String);

/// The longest address [`Email::parse`] accepts (`docs/data-model.md`,
/// `users`): the RFC 5321 maximum for a reverse or forward path.
pub const EMAIL_MAX_CHARS: usize = 254;

impl Email {
    /// Trim and lower-case `raw`, then accept it when it is exactly one `@`
    /// with a non-empty local part, a non-empty domain and no whitespace.
    pub fn parse(raw: &str) -> UserResult<Self> {
        let normalised = raw.trim().to_lowercase();

        // The longest address an SMTP path can carry (RFC 5321). The column is
        // `TEXT`, so without this an invite could be created for an address no
        // provider would ever accept.
        if normalised.chars().count() > EMAIL_MAX_CHARS {
            return Err(UserError::InvalidEmail);
        }

        let Some((local, domain)) = normalised.split_once('@') else {
            return Err(UserError::InvalidEmail);
        };
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return Err(UserError::InvalidEmail);
        }
        // Interior whitespace survives the trim and would be stored verbatim.
        if normalised.chars().any(char::is_whitespace) {
            return Err(UserError::InvalidEmail);
        }

        Ok(Self(normalised))
    }

    /// The normalised address, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Email> for String {
    fn from(email: Email) -> Self {
        email.0
    }
}

impl std::fmt::Display for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A `users` row, column for column (`docs/data-model.md`, `users`).
///
/// Serialising this row *is* the API-facing `User` — `{ id, username, email,
/// admin, must_change_password, notify_email, created_at }` (`SPEC.md`,
/// "Users") — so routes return the row itself rather than copying it into a
/// DTO. What makes that safe is that the three columns outside that list are
/// `#[serde(skip)]`: `password_hash` and `auth_version` must never leave the
/// process, and `updated_at` is internal bookkeeping the contract does not
/// mention. A test asserts the exact key set, so adding a column without
/// deciding about it fails rather than leaking it.
///
/// There is no `Deserialize`: a `User` only ever comes out of the database,
/// and deriving it would make an empty `password_hash` constructible from JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    #[serde(skip)]
    pub password_hash: String,
    #[serde(skip)]
    pub auth_version: i64,
    pub must_change_password: bool,
    pub admin: bool,
    pub notify_email: bool,
    pub created_at: DateTime<Utc>,
    #[serde(skip)]
    pub updated_at: DateTime<Utc>,
}

/// The caller-supplied half of a new user.
///
/// The id is generated up front so the caller knows it before the insert. The
/// repository fills in `auth_version`, `notify_email` and the timestamps from
/// the column defaults. Hashing the chosen [`Password`] into `password_hash`
/// is the authentication epic's job; this struct takes the finished PHC string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub id: Uuid,
    pub username: Username,
    pub email: Email,
    pub password_hash: String,
    pub admin: bool,
    pub must_change_password: bool,
}

impl NewUser {
    /// A user with a fresh id, no admin rights and no forced password change.
    ///
    /// The remaining fields are public: callers set what they were given.
    pub fn new(username: &str, email: &str, password_hash: String) -> UserResult<Self> {
        Ok(Self {
            id: Uuid::new_v4(),
            username: Username::parse(username)?,
            email: Email::parse(email)?,
            password_hash,
            admin: false,
            must_change_password: false,
        })
    }
}

/// The fields `PUT /users/{id}` and `PATCH /users/me` can change (`SPEC.md`,
/// "Users").
///
/// Every field is optional and `None` means "leave it alone", so one statement
/// serves both endpoints. The password, `auth_version` and
/// `must_change_password` are deliberately absent: they move together in the
/// authentication epic's password transaction, never through a partial update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserUpdate {
    pub username: Option<Username>,
    pub admin: Option<bool>,
    pub notify_email: Option<bool>,
}

impl UserUpdate {
    /// Whether this update would change anything at all.
    pub fn is_empty(&self) -> bool {
        self.username.is_none() && self.admin.is_none() && self.notify_email.is_none()
    }
}

/// A `refresh_tokens` row, column for column (`docs/data-model.md`,
/// `refresh_tokens`).
///
/// `token_hash` is the SHA-256 hex of the raw token; the raw token exists only
/// in the response cookie and cannot be reconstructed from the row. Not
/// serialisable: no endpoint returns a refresh-token row.
///
/// There is no `is_usable` here. Whether a token may still be exchanged —
/// unrevoked and unexpired — is decided by the lookup query,
/// [`RefreshTokenRepository::find_usable_by_hash_for_user`](crate::repositories::RefreshTokenRepository::find_usable_by_hash_for_user),
/// which runs inside the caller's locked transaction; a predicate on this
/// struct would be a second statement of the same rule, applicable to a row
/// read before the lock (`docs/data-model.md`, `refresh_tokens`).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// A `user_invites` row, column for column (`docs/data-model.md`,
/// `user_invites`).
///
/// This row *is* the API-facing `Invite` — `{ id, email, admin, invited_by,
/// expires_at, created_at }` (`SPEC.md`, "Users"): the three columns that are
/// not in that shape are `#[serde(skip)]`, so a row cannot leak a token hash
/// into a response even when a route serialises it directly. `token_hash` is
/// the SHA-256 hex of the raw token in the emailed link and the raw token is
/// never stored (`CLAUDE.md`, rule 3); `accepted_at` and `accepted_user_id`
/// are internal bookkeeping that only ever describes an invite the API no
/// longer lists.
///
/// There is no `Deserialize`: an invite only ever comes out of the database,
/// and no `is_usable`: whether an invite is still open — unaccepted and
/// unexpired — is in the `WHERE` clause of the lookups that find one
/// (`docs/data-model.md`, `user_invites`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct UserInvite {
    pub id: Uuid,
    pub email: String,
    #[serde(skip)]
    pub token_hash: String,
    pub admin: bool,
    pub invited_by: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    #[serde(skip)]
    pub accepted_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub accepted_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// A `password_reset_tokens` row, column for column (`docs/data-model.md`,
/// `password_reset_tokens`).
///
/// Single-use: a successful password change or reset sets `used_at` on every
/// outstanding row for that user. Not serialisable; no endpoint returns one.
///
/// No `is_usable`: unused and unexpired is part of the lookup query
/// (`docs/data-model.md`, `password_reset_tokens`).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PasswordResetToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a credential: a syntactically valid PHC string with a fake body,
    /// used wherever a test needs a `password_hash` (`CLAUDE.md`, rule 3).
    const FAKE_HASH: &str = "$argon2id$fake$hash";

    #[test]
    fn every_input_rejection_is_a_bad_request_with_a_message() {
        for error in [
            UserError::InvalidUsername,
            UserError::InvalidPassword,
            UserError::InvalidEmail,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }
    }

    #[test]
    fn a_hashing_failure_is_this_process_failing_not_the_caller() {
        assert_eq!(
            UserError::Hash.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a failure of our own hasher must not be reported as bad input"
        );
        assert_eq!(
            Error::from(UserError::Hash).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn the_crate_error_delegates_to_the_model() {
        let error = Error::from(UserError::InvalidUsername);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), UserError::InvalidUsername.to_string());
    }

    #[test]
    fn username_is_trimmed() {
        let username = Username::parse("  ada \n").unwrap();
        assert_eq!(username.as_str(), "ada");
        assert_eq!(username.to_string(), "ada");
        assert_eq!(String::from(username), "ada");
    }

    #[test]
    fn username_accepts_three_and_thirty_two_characters() {
        assert_eq!(Username::parse(&"u".repeat(3)).unwrap().as_str(), "uuu");
        let longest = "u".repeat(MAX_USERNAME_CHARS);
        assert_eq!(Username::parse(&longest).unwrap().as_str(), longest);
    }

    #[test]
    fn username_rejects_two_and_thirty_three_characters() {
        assert_eq!(
            Username::parse(&"u".repeat(MIN_USERNAME_CHARS - 1)),
            Err(UserError::InvalidUsername)
        );
        assert_eq!(
            Username::parse(&"u".repeat(MAX_USERNAME_CHARS + 1)),
            Err(UserError::InvalidUsername)
        );
    }

    #[test]
    fn username_rejects_empty_and_whitespace_only() {
        assert_eq!(Username::parse(""), Err(UserError::InvalidUsername));
        assert_eq!(Username::parse("   \t\n "), Err(UserError::InvalidUsername));
        // Two characters and a space: trimming decides, not the raw length.
        assert_eq!(Username::parse(" ad "), Err(UserError::InvalidUsername));
    }

    #[test]
    fn username_counts_characters_not_bytes() {
        assert_eq!(Username::parse("åäö").unwrap().as_str(), "åäö");
        assert!(Username::parse(&"å".repeat(MAX_USERNAME_CHARS)).is_ok());
        assert_eq!(
            Username::parse(&"å".repeat(MAX_USERNAME_CHARS + 1)),
            Err(UserError::InvalidUsername)
        );
    }

    #[test]
    fn username_keeps_its_case() {
        assert_eq!(Username::parse("Ada").unwrap().as_str(), "Ada");
    }

    #[test]
    fn password_accepts_ten_and_a_hundred_and_twenty_eight_characters() {
        let shortest = "p".repeat(MIN_PASSWORD_CHARS);
        assert_eq!(Password::parse(&shortest).unwrap().expose(), shortest);
        let longest = "p".repeat(MAX_PASSWORD_CHARS);
        assert_eq!(Password::parse(&longest).unwrap().expose(), longest);
    }

    #[test]
    fn password_rejects_nine_and_a_hundred_and_twenty_nine_characters() {
        assert_eq!(
            Password::parse(&"p".repeat(MIN_PASSWORD_CHARS - 1)),
            Err(UserError::InvalidPassword)
        );
        assert_eq!(
            Password::parse(&"p".repeat(MAX_PASSWORD_CHARS + 1)),
            Err(UserError::InvalidPassword)
        );
        assert_eq!(Password::parse(""), Err(UserError::InvalidPassword));
    }

    #[test]
    fn password_counts_characters_not_bytes() {
        assert!(Password::parse(&"å".repeat(MAX_PASSWORD_CHARS)).is_ok());
        assert_eq!(
            Password::parse(&"å".repeat(MAX_PASSWORD_CHARS + 1)),
            Err(UserError::InvalidPassword)
        );
    }

    #[test]
    fn password_keeps_whitespace_exactly_as_typed() {
        let raw = "  a pass phrase  ";
        assert_eq!(Password::parse(raw).unwrap().expose(), raw);
    }

    #[test]
    fn password_debug_never_shows_the_password() {
        let password = Password::parse("correct horse battery").unwrap();
        let rendered = format!("{password:?}");
        assert_eq!(rendered, "Password(<redacted>)");
        assert!(!rendered.contains("horse"), "leaked: {rendered}");

        // And when it is nested inside something else's `Debug`.
        let rendered = format!("{:?}", Some(("login", password)));
        assert!(!rendered.contains("horse"), "leaked: {rendered}");
    }

    #[test]
    fn password_rejection_never_echoes_the_password() {
        let message = UserError::InvalidPassword.to_string();
        assert_eq!(message, "password must be 10-128 characters");
    }

    #[test]
    fn email_is_trimmed_and_lower_cased() {
        let email = Email::parse("  Ada@Example.COM \n").unwrap();
        assert_eq!(email.as_str(), "ada@example.com");
        assert_eq!(email.to_string(), "ada@example.com");
        assert_eq!(String::from(email), "ada@example.com");
    }

    #[test]
    fn email_normalisation_makes_variants_collide() {
        let one = Email::parse("ADA@EXAMPLE.COM").unwrap();
        let two = Email::parse(" ada@example.com ").unwrap();
        assert_eq!(one, two);
        assert_eq!(one.as_str(), two.as_str());
    }

    #[test]
    fn email_accepts_the_seeded_admin_address() {
        assert_eq!(
            Email::parse("admin@localhost").unwrap().as_str(),
            "admin@localhost"
        );
    }

    #[test]
    fn email_rejects_a_missing_or_repeated_at_sign() {
        assert_eq!(
            Email::parse("ada.example.com"),
            Err(UserError::InvalidEmail)
        );
        assert_eq!(
            Email::parse("ada@@example.com"),
            Err(UserError::InvalidEmail)
        );
        assert_eq!(
            Email::parse("ada@example@com"),
            Err(UserError::InvalidEmail)
        );
    }

    #[test]
    fn email_rejects_empty_local_and_domain_parts() {
        assert_eq!(Email::parse("@example.com"), Err(UserError::InvalidEmail));
        assert_eq!(Email::parse("ada@"), Err(UserError::InvalidEmail));
        assert_eq!(Email::parse("@"), Err(UserError::InvalidEmail));
        assert_eq!(Email::parse(""), Err(UserError::InvalidEmail));
        assert_eq!(Email::parse("   "), Err(UserError::InvalidEmail));
    }

    #[test]
    fn email_rejects_interior_whitespace() {
        assert_eq!(
            Email::parse("ada b@example.com"),
            Err(UserError::InvalidEmail)
        );
        assert_eq!(
            Email::parse("ada@exa mple.com"),
            Err(UserError::InvalidEmail)
        );
        assert_eq!(
            Email::parse("ada@example.com\tx"),
            Err(UserError::InvalidEmail)
        );
    }

    #[test]
    fn email_rejects_an_address_longer_than_the_smtp_maximum() {
        let domain = "@example.test";
        let local = "a".repeat(EMAIL_MAX_CHARS - domain.len());
        let longest = format!("{local}{domain}");
        assert_eq!(longest.len(), EMAIL_MAX_CHARS);
        assert_eq!(
            Email::parse(&longest).map(|parsed| parsed.as_str().len()),
            Ok(EMAIL_MAX_CHARS)
        );

        let too_long = format!("a{longest}");
        assert_eq!(Email::parse(&too_long), Err(UserError::InvalidEmail));
    }

    #[test]
    fn a_new_user_starts_with_the_documented_defaults() {
        let user = NewUser::new("  Ada ", " Ada@Example.com ", FAKE_HASH.to_string()).unwrap();
        assert_eq!(user.username.as_str(), "Ada");
        assert_eq!(user.email.as_str(), "ada@example.com");
        assert_eq!(user.password_hash, FAKE_HASH);
        assert!(!user.admin);
        assert!(!user.must_change_password);
    }

    #[test]
    fn a_new_user_rejects_an_invalid_username_or_email() {
        assert_eq!(
            NewUser::new("ad", "ada@example.com", FAKE_HASH.to_string()).unwrap_err(),
            UserError::InvalidUsername
        );
        assert_eq!(
            NewUser::new("ada", "nope", FAKE_HASH.to_string()).unwrap_err(),
            UserError::InvalidEmail
        );
    }

    #[test]
    fn an_empty_update_changes_nothing() {
        assert!(UserUpdate::default().is_empty());
        assert!(
            !UserUpdate {
                notify_email: Some(false),
                ..UserUpdate::default()
            }
            .is_empty()
        );
    }

    fn a_user() -> User {
        User {
            id: Uuid::nil(),
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
    fn a_user_row_never_serialises_its_hash_or_auth_version() {
        let json = serde_json::to_value(a_user()).unwrap();

        assert!(json.get("password_hash").is_none(), "leaked: {json}");
        assert!(json.get("auth_version").is_none(), "leaked: {json}");
        assert_eq!(json["username"], "ada");
        assert_eq!(json["email"], "ada@example.com");
        assert_eq!(json["admin"], true);
        assert_eq!(json["must_change_password"], true);
        assert_eq!(json["notify_email"], false);
    }

    #[test]
    fn an_invite_row_serialises_as_the_invite_dto() {
        let invite = UserInvite {
            id: Uuid::nil(),
            email: "ada@example.com".into(),
            token_hash: "fake-hash".into(),
            admin: true,
            invited_by: Some(Uuid::nil()),
            expires_at: Utc::now(),
            accepted_at: Some(Utc::now()),
            accepted_user_id: Some(Uuid::nil()),
            created_at: Utc::now(),
        };

        let json = serde_json::to_value(&invite).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("an invite serialises as an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        // Exactly `SPEC.md`, "Users": no token hash, no acceptance bookkeeping.
        assert_eq!(
            keys,
            [
                "admin",
                "created_at",
                "email",
                "expires_at",
                "id",
                "invited_by"
            ]
        );
    }

    #[test]
    fn a_user_serialises_to_exactly_the_documented_fields() {
        // `SPEC.md`, "Users": `User = { id, username, email, admin,
        // must_change_password, notify_email, created_at }` — no more, no less.
        const SPEC_FIELDS: &[&str] = &[
            "admin",
            "created_at",
            "email",
            "id",
            "must_change_password",
            "notify_email",
            "username",
        ];

        let json = serde_json::to_value(a_user()).unwrap();
        let object = json.as_object().expect("a user serialises to an object");

        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();

        assert_eq!(keys, SPEC_FIELDS, "the serialised shape is: {json}");
    }

    #[test]
    fn a_password_hashes_to_a_phc_string_and_verifies() {
        let password = "correct horse battery";

        let hash = hash_password(password).unwrap();

        assert!(
            hash.starts_with("$argon2id$v=19$"),
            "unexpected hash: {hash}"
        );
        assert!(!hash.contains(password), "leaked: {hash}");
        assert!(verify_password(&hash, password));
    }

    #[test]
    fn the_wrong_password_does_not_verify() {
        let hash = hash_password("correct horse battery").unwrap();

        assert!(!verify_password(&hash, "correct horse batterY"));
        assert!(!verify_password(&hash, "correct horse batter"));
        assert!(!verify_password(&hash, ""));
    }

    #[test]
    fn the_same_password_hashes_differently_every_time() {
        let one = hash_password("correct horse battery").unwrap();
        let two = hash_password("correct horse battery").unwrap();

        assert_ne!(one, two, "the salt is not fresh");
        // Both still verify: the salt travels inside the PHC string.
        assert!(verify_password(&one, "correct horse battery"));
        assert!(verify_password(&two, "correct horse battery"));
    }

    #[test]
    fn a_malformed_hash_verifies_as_false_and_never_panics() {
        for hash in [
            "",
            "   ",
            FAKE_HASH,
            "not a phc string",
            "$argon2id$",
            // Truncated after the salt.
            "$argon2id$v=19$m=19456,t=2,p=1$LdkhVNG/wlqQnG2ibpyNiA",
            // A parameter that is not a number.
            "$argon2id$v=19$m=wat,t=2,p=1$LdkhVNG/wlqQnG2ibpyNiA$8Bl44TOasplKBxanrfHFXTjyUXCZ4EAe30zLkRQmscE",
            // Another algorithm's PHC string.
            "$scrypt$ln=16,r=8,p=1$LdkhVNG/wlqQnG2ibpyNiA$8Bl44TOasplKBxanrfHFXTjyUXCZ4EAe30zLkRQmscE",
        ] {
            assert!(
                !verify_password(hash, "correct horse battery"),
                "accepted a malformed hash: {hash}"
            );
        }
    }

    #[test]
    fn the_seeded_administrator_hash_still_verifies() {
        // The row the `users` migration seeds, with the default password
        // `README.md`, "Start" documents and the first login is forced to
        // change. Not a credential of any deployment: it is public, fixed and
        // `must_change_password` is true (ADR 0024). Here it pins the Argon2
        // parameters — a default this crate moves away from would stop this
        // build from verifying the hashes already in every database.
        const SEEDED: &str = "$argon2id$v=19$m=19456,t=2,p=1$LdkhVNG/wlqQnG2ibpyNiA$8Bl44TOasplKBxanrfHFXTjyUXCZ4EAe30zLkRQmscE";

        assert!(verify_password(SEEDED, "changeme"));
        assert!(!verify_password(SEEDED, "changeme "));
    }
}
