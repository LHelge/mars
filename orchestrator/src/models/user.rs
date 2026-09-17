//! Users and the three authentication tables that hang off them.
//!
//! `docs/data-model.md`, "Users and authentication" is the column contract;
//! `SPEC.md`, "Users" is the field contract the API exposes and `SPEC.md`,
//! "User-facing features" is where the 10–128 character password rule comes
//! from. Validation lives here, SQL lives in `repositories/` (`CLAUDE.md`,
//! "Backend conventions").
//!
//! Nothing in this module hashes, compares or issues a credential: that is the
//! authentication epic's. What it does guarantee is the *stored* form of the
//! three fields a unique index or a login depends on — the username, the email
//! and the password the caller chose — so a row can never reach the database
//! untrimmed, differently cased or out of bounds.

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
}

impl UserError {
    /// The HTTP status this rejection maps to.
    ///
    /// Every variant is malformed input, so every variant is 400. A username
    /// or email that is well formed but already taken is decided against the
    /// database and surfaces as [`Error::Conflict`] from the repository
    /// instead (`SPEC.md`, "Users").
    pub fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
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
/// authentication"). Anything beyond the shape below is left to delivery —
/// addresses arrive from an invite that was actually sent, so a syntactic
/// check here only has to keep the unique index honest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Email(String);

impl Email {
    /// Trim and lower-case `raw`, then accept it when it is exactly one `@`
    /// with a non-empty local part, a non-empty domain and no whitespace.
    pub fn parse(raw: &str) -> UserResult<Self> {
        let normalised = raw.trim().to_lowercase();

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
/// `password_hash` and `auth_version` are `#[serde(skip)]` so that a row can
/// never be serialised into a response by accident. The API-facing `User` —
/// `{ id, username, email, admin, must_change_password, notify_email,
/// created_at }` (`SPEC.md`, "Users") — is assembled by the routes from this
/// row; it also leaves out `updated_at`, which is internal bookkeeping.
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
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl RefreshToken {
    /// Whether this token can still be exchanged at `now`.
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }
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
/// There is no `Deserialize`: an invite only ever comes out of the database.
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

impl UserInvite {
    /// Whether this invite can still be accepted at `now`.
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        self.accepted_at.is_none() && self.expires_at > now
    }
}

/// A `password_reset_tokens` row, column for column (`docs/data-model.md`,
/// `password_reset_tokens`).
///
/// Single-use: a successful password change or reset sets `used_at` on every
/// outstanding row for that user. Not serialisable; no endpoint returns one.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PasswordResetToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl PasswordResetToken {
    /// Whether this token can still be consumed at `now`.
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        self.used_at.is_none() && self.expires_at > now
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    /// Not a credential: a syntactically valid PHC string with a fake body,
    /// used wherever a test needs a `password_hash` (`CLAUDE.md`, rule 3).
    const FAKE_HASH: &str = "$argon2id$fake$hash";

    #[test]
    fn every_error_is_a_bad_request_with_a_message() {
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

    #[test]
    fn a_user_row_never_serialises_its_hash_or_auth_version() {
        let user = User {
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
        };

        let json = serde_json::to_value(&user).unwrap();
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
    fn a_token_is_usable_only_while_unspent_and_unexpired() {
        let now = Utc::now();
        let hour = TimeDelta::try_hours(1).unwrap();

        let mut token = RefreshToken {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            token_hash: "fake-hash".into(),
            expires_at: now + hour,
            revoked_at: None,
            created_at: now,
        };
        assert!(token.is_usable(now));
        token.expires_at = now - hour;
        assert!(!token.is_usable(now));
        token.expires_at = now + hour;
        token.revoked_at = Some(now);
        assert!(!token.is_usable(now));

        let mut reset = PasswordResetToken {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            token_hash: "fake-hash".into(),
            expires_at: now + hour,
            used_at: None,
            created_at: now,
        };
        assert!(reset.is_usable(now));
        reset.used_at = Some(now);
        assert!(!reset.is_usable(now));

        let mut invite = UserInvite {
            id: Uuid::new_v4(),
            email: "ada@example.com".into(),
            token_hash: "fake-hash".into(),
            admin: false,
            invited_by: None,
            expires_at: now + hour,
            accepted_at: None,
            accepted_user_id: None,
            created_at: now,
        };
        assert!(invite.is_usable(now));
        invite.accepted_at = Some(now);
        assert!(!invite.is_usable(now));
    }
}
