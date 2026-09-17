//! Secrets: the envelope-encrypted values and the audit of their use.
//!
//! `docs/data-model.md`, "Secrets" is the column contract, `ARCHITECTURE.md`,
//! "Secrets" is the design and `SPEC.md`, "Secrets" is what the API exposes.
//! Validation lives here, SQL lives in `repositories/secrets.rs` (`CLAUDE.md`,
//! "Backend conventions").
//!
//! What this module does **not** do is any cryptography. It has no master key,
//! it never builds the additional authenticated data and it never decrypts:
//! the four byte columns arrive already encrypted, as an [`EncryptedValue`],
//! and leave the same way. Producing one is the envelope-crypto epic's job
//! (`ARCHITECTURE.md`, "Secrets"). What this module does guarantee is the
//! *shape* of a row the unique index and the `CHECK` depend on — an
//! environment-variable style [`SecretName`] and a [`ScopeRef`] whose
//! `scope_id` is present exactly when the scope is not `global` — so a row can
//! never reach the database in a form the schema would have to reject.
//!
//! **Two error types, deliberately.** [`SecretError`] here is a *caller's*
//! mistake — a malformed name, an impossible scope — and answers 400.
//! `crate::secrets::SecretsError` (plural) is the keyring's own failure —
//! unreadable or missing master key material — and answers 500. A request can
//! produce the first; only the operator's configuration produces the second.
//!
//! Nothing in this module logs, formats or serialises a value: the four byte
//! columns are `#[serde(skip)]` and [`Secret`]'s `Debug` redacts them, so a
//! ciphertext cannot reach a log line through a `?` field or a derived `Debug`
//! on something that happens to hold a row (`CLAUDE.md`, rule 3).

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions"). Models report
// their own error rather than the crate-wide one, so the glob is here for the
// doc links and for what these models grow into.
use crate::models::agent_profile::is_secret_name;
#[allow(unused_imports)]
use crate::prelude::*;

/// Every way a secret model can reject its input.
///
/// `Display` is the message the API returns in `{ status, error }`, so no
/// variant carries internal detail and none of them ever echoes a value —
/// there is no variant that could, because a value never reaches this module
/// in the clear (`CLAUDE.md`, rule 3). [`SecretError::status`] is the HTTP
/// status the crate-wide [`Error`] delegates to (`ARCHITECTURE.md`,
/// "Orchestrator internals").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The name did not match `^[A-Z][A-Z0-9_]{0,127}$`.
    #[error("secret name must match ^[A-Z][A-Z0-9_]{{0,127}}$")]
    InvalidName,
    /// A `global` secret was given a `scope_id`, or a `user` or `project` one
    /// was not.
    #[error("a global secret has no scope_id and a user or project secret needs one")]
    InvalidScope,
    /// The stored `purpose` was not one of the documented values.
    #[error("secret use purpose must be launch or git")]
    InvalidPurpose,
    /// The value was empty or longer than [`MAX_SECRET_VALUE_BYTES`].
    ///
    /// The message names the limit and never the value — there is nothing in
    /// the value that belongs in a response or a log line (`CLAUDE.md`, rule
    /// 3).
    #[error("secret value must be between 1 and {MAX_SECRET_VALUE_BYTES} bytes")]
    InvalidValue,
}

/// The largest value the API accepts, in bytes of UTF-8.
///
/// A secret becomes an environment variable in the container, and 64 KiB is
/// generously above the largest credential anybody stores in one — a PEM
/// private key is a few kilobytes — while keeping a request body that has to
/// be sealed in memory bounded. The limit is on bytes rather than characters
/// because that is what is encrypted and what an environment holds.
pub const MAX_SECRET_VALUE_BYTES: usize = 65_536;

/// Accept a secret value of at least one byte and at most
/// [`MAX_SECRET_VALUE_BYTES`].
///
/// Deliberately not a newtype the way [`SecretName`] is: a value is never
/// stored, compared or rendered in the clear, so it exists only as the
/// `Zeroizing<String>` the service seals and drops, and a wrapper around it
/// would be one more place holding a plaintext credential for no gain
/// (`ARCHITECTURE.md`, "Secrets", Credential handling). Nothing is trimmed:
/// leading or trailing whitespace can be significant in a credential, so a
/// value is stored exactly as it arrived.
///
/// An empty value is rejected here rather than in the cipher, which round
/// trips it happily: an empty environment variable is almost always a client
/// that failed to read its own configuration, and storing it would inject an
/// empty credential into every session that resolves the name.
pub fn validate_secret_value(value: &str) -> SecretResult<()> {
    if value.is_empty() || value.len() > MAX_SECRET_VALUE_BYTES {
        return Err(SecretError::InvalidValue);
    }

    Ok(())
}

impl SecretError {
    /// The HTTP status this rejection maps to.
    ///
    /// Every variant is malformed input, so every variant is 400. A name that
    /// is well formed but already taken in its scope is decided against the
    /// database and surfaces as [`Error::Conflict`] from the repository
    /// instead (`SPEC.md`, "Secrets": 409 if exists).
    pub fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// The result type the secret models return.
///
/// The prelude's `Result` is the crate-wide one; models report their own error
/// and let `?` widen it at the call site.
pub type SecretResult<T> = std::result::Result<T, SecretError>;

/// Which population a secret belongs to (`docs/data-model.md`, "Enums",
/// `secret_scope`).
///
/// Resolution at launch walks them in the order `Global`, `Project`, `User`
/// and the last row found wins (`ARCHITECTURE.md`, "Secrets", Resolution at
/// launch). That order is the resolver's, not this enum's; nothing here
/// depends on the declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "secret_scope", rename_all = "snake_case")]
pub enum SecretScope {
    /// Visible to every session; `scope_id` is NULL.
    Global,
    /// The launching user's own; `scope_id` is a `users.id`.
    User,
    /// Every session of one project; `scope_id` is a `projects.id`.
    Project,
}

impl SecretScope {
    /// The stored spelling, which is also the first field of the additional
    /// authenticated data the envelope-crypto epic builds.
    pub fn as_str(&self) -> &'static str {
        match self {
            SecretScope::Global => "global",
            SecretScope::User => "user",
            SecretScope::Project => "project",
        }
    }
}

impl std::fmt::Display for SecretScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A validated `(scope, scope_id)` pair.
///
/// The table's `CHECK ((scope = 'global') = (scope_id IS NULL))` exists
/// because either half alone makes resolution ambiguous: a global secret with
/// a target nobody looks at, or a user secret belonging to no user
/// (`docs/data-model.md`, `secrets`). Carrying the pair as one type means a
/// repository call cannot be given a combination the database would reject,
/// and cannot look one up either.
///
/// Deliberately not a foreign key in either direction — the scope decides
/// which table `scope_id` points at — so existence is the caller's to check
/// and [`crate::repositories::SecretRepository::list_orphans`] is what cleans
/// up after a deleted user or project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ScopeRef {
    scope: SecretScope,
    scope_id: Option<Uuid>,
}

impl ScopeRef {
    /// Accept `scope` and `scope_id` when `scope` is
    /// [`SecretScope::Global`] exactly if `scope_id` is `None`.
    pub fn new(scope: SecretScope, scope_id: Option<Uuid>) -> SecretResult<Self> {
        if (scope == SecretScope::Global) != scope_id.is_none() {
            return Err(SecretError::InvalidScope);
        }
        Ok(Self { scope, scope_id })
    }

    /// The global scope.
    pub fn global() -> Self {
        Self {
            scope: SecretScope::Global,
            scope_id: None,
        }
    }

    /// One user's scope.
    pub fn user(user_id: Uuid) -> Self {
        Self {
            scope: SecretScope::User,
            scope_id: Some(user_id),
        }
    }

    /// One project's scope.
    pub fn project(project_id: Uuid) -> Self {
        Self {
            scope: SecretScope::Project,
            scope_id: Some(project_id),
        }
    }

    /// Which population this is.
    pub fn scope(&self) -> SecretScope {
        self.scope
    }

    /// The user or project id, or `None` for [`SecretScope::Global`].
    pub fn scope_id(&self) -> Option<Uuid> {
        self.scope_id
    }
}

/// A validated secret name: `^[A-Z][A-Z0-9_]{0,127}$`.
///
/// Case is significant and never normalised: the name becomes an environment
/// variable in the container and is part of the additional authenticated data,
/// so `secrets_scope_scope_id_name_key` compares exactly the bytes that were
/// entered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SecretName(String);

impl SecretName {
    /// Accept `raw` when it matches `^[A-Z][A-Z0-9_]{0,127}$`.
    ///
    /// Nothing is trimmed: whitespace is not in the pattern, so a name with
    /// any is rejected rather than quietly changed into a different name.
    pub fn parse(raw: &str) -> SecretResult<Self> {
        if is_secret_name(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(SecretError::InvalidName)
        }
    }

    /// The name, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<SecretName> for String {
    fn from(name: SecretName) -> Self {
        name.0
    }
}

impl std::fmt::Display for SecretName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The four encrypted columns and the master-key version that wrapped them.
///
/// Every write that changes what is stored moves all five together: replacing
/// a value re-encrypts it, and renaming re-encrypts it too, because the name
/// is part of the additional authenticated data (`docs/data-model.md`,
/// `secrets`). Bundling them is what keeps a caller from updating a ciphertext
/// and leaving the nonce behind.
///
/// Built by the envelope-crypto epic and never inspected here. `Debug` and
/// serialisation are both refused: there is nothing in this struct that is
/// safe to print (`CLAUDE.md`, rule 3).
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedValue {
    /// AES-256-GCM ciphertext of the value, including the 16-byte tag.
    pub ciphertext: Vec<u8>,
    /// 12-byte nonce for `ciphertext`.
    pub nonce: Vec<u8>,
    /// The per-row data key, AES-256-GCM wrapped by the master key.
    pub data_key_wrapped: Vec<u8>,
    /// 12-byte nonce for the wrap.
    pub data_key_nonce: Vec<u8>,
    /// Which master key wrapped `data_key_wrapped`.
    pub key_version: i32,
}

impl std::fmt::Debug for EncryptedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key version is operational detail and safe; nothing else is.
        f.debug_struct("EncryptedValue")
            .field("ciphertext", &"<redacted>")
            .field("nonce", &"<redacted>")
            .field("data_key_wrapped", &"<redacted>")
            .field("data_key_nonce", &"<redacted>")
            .field("key_version", &self.key_version)
            .finish()
    }
}

/// A `secrets` row, column for column (`docs/data-model.md`, `secrets`).
///
/// The four byte columns are `#[serde(skip)]` and redacted by `Debug`: the API
/// is write-only and no response ever contains a value (`SPEC.md`, "Secrets"),
/// so a row that reached a response body or a log line would be a defect
/// whichever way it got there. [`SecretMeta`] is the shape the API actually
/// returns.
///
/// There is no `Deserialize`: a `Secret` only ever comes out of the database.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Secret {
    pub id: Uuid,
    pub scope: SecretScope,
    pub scope_id: Option<Uuid>,
    pub name: String,
    #[serde(skip)]
    pub ciphertext: Vec<u8>,
    #[serde(skip)]
    pub nonce: Vec<u8>,
    #[serde(skip)]
    pub data_key_wrapped: Vec<u8>,
    #[serde(skip)]
    pub data_key_nonce: Vec<u8>,
    pub key_version: i32,
    pub orchestrator_only: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .field("scope_id", &self.scope_id)
            .field("name", &self.name)
            .field("ciphertext", &"<redacted>")
            .field("nonce", &"<redacted>")
            .field("data_key_wrapped", &"<redacted>")
            .field("data_key_nonce", &"<redacted>")
            .field("key_version", &self.key_version)
            .field("orchestrator_only", &self.orchestrator_only)
            .field("created_by", &self.created_by)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl Secret {
    /// The validated `(scope, scope_id)` pair of this row.
    ///
    /// The `CHECK` guarantees the pair is well formed, so a stored row always
    /// produces one; the `expect` documents that rather than making every
    /// caller handle a case the database cannot produce.
    pub fn scope_ref(&self) -> ScopeRef {
        ScopeRef::new(self.scope, self.scope_id)
            .expect("a stored row satisfies the scope/scope_id check")
    }

    /// The five encrypted columns of this row, as the crypto layer takes them.
    ///
    /// A [`Secret`] is the whole row and the cipher wants only the envelope,
    /// so the columns are gathered here rather than in each of the resolver,
    /// the git credential helpers and the secrets service, which each had
    /// their own private copy of exactly this function. The clone is of
    /// ciphertext and wrapping, never of a plaintext — this module still
    /// decrypts nothing.
    ///
    /// An inherent method rather than `impl From<&Secret> for EncryptedValue`:
    /// every call site already holds a `&Secret` and reads better as
    /// `row.encrypted_value()` than as `EncryptedValue::from(row)`, and a
    /// `From` would also make the conversion available by inference in places
    /// that did not ask for it, which is not something a ciphertext should get.
    pub fn encrypted_value(&self) -> EncryptedValue {
        EncryptedValue {
            ciphertext: self.ciphertext.clone(),
            nonce: self.nonce.clone(),
            data_key_wrapped: self.data_key_wrapped.clone(),
            data_key_nonce: self.data_key_nonce.clone(),
            key_version: self.key_version,
        }
    }
}

/// The caller-supplied half of a new secret.
///
/// The id is generated up front so the caller knows it before the insert. The
/// repository fills in the timestamps from the column defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSecret {
    pub id: Uuid,
    pub scope: ScopeRef,
    pub name: SecretName,
    pub value: EncryptedValue,
    pub orchestrator_only: bool,
    pub created_by: Option<Uuid>,
}

impl NewSecret {
    /// A secret with a fresh id, injected into containers, created by nobody.
    ///
    /// The remaining fields are public: callers set what they were given.
    pub fn new(scope: ScopeRef, name: SecretName, value: EncryptedValue) -> Self {
        Self {
            id: Uuid::new_v4(),
            scope,
            name,
            value,
            orchestrator_only: false,
            created_by: None,
        }
    }
}

/// What `GET /secrets` and every other secret endpoint return (`SPEC.md`,
/// "Secrets").
///
/// A separate struct rather than a `#[serde(skip)]`-ed [`Secret`], so that
/// adding a column to the row cannot accidentally widen the response, and so
/// that `last_used_at` — which is an aggregate over `secret_uses`, not a
/// column — has somewhere to live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretMeta {
    pub id: Uuid,
    pub scope: SecretScope,
    pub scope_id: Option<Uuid>,
    pub name: String,
    pub orchestrator_only: bool,
    pub key_version: i32,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The newest `secret_uses.at` for this secret, or `None` if it has never
    /// been used.
    pub last_used_at: Option<DateTime<Utc>>,
}

/// One stored row per distinct `key_version`, for the startup check.
///
/// [`crate::secrets::SecretsKeyring::verify_against_db`] unwraps one sample
/// per version present in the table before the orchestrator serves anything,
/// so a master key missing from the environment is found at boot rather than
/// at a session launch (`ARCHITECTURE.md`, "Secrets", Keyring).
///
/// The two byte fields are the wrapping of a data key rather than a value,
/// but they are still key material: `Debug` shows the version and nothing
/// else, and there is no `Serialize` (`CLAUDE.md`, rule 3).
#[derive(Clone, PartialEq, Eq)]
pub struct KeyVersionSample {
    /// The version every row in this group is wrapped under.
    pub key_version: i32,
    /// The sampled row's wrapped data key.
    pub data_key_wrapped: Vec<u8>,
    /// The nonce that wrap used.
    pub data_key_nonce: Vec<u8>,
}

impl std::fmt::Debug for KeyVersionSample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyVersionSample")
            .field("key_version", &self.key_version)
            .field("data_key_wrapped", &"<redacted>")
            .field("data_key_nonce", &"<redacted>")
            .finish()
    }
}

/// Why a secret was read (`docs/data-model.md`, `secret_uses`).
///
/// Stored as `TEXT` rather than an enum type: the document lists no `CHECK`
/// for the column, so the value set is this Rust type's and adding one needs
/// no migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum SecretUsePurpose {
    /// Injected into a session container at launch, including a relaunch of a
    /// parked session.
    Launch,
    /// Read by the git credential provider.
    Git,
}

impl SecretUsePurpose {
    /// The stored spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            SecretUsePurpose::Launch => "launch",
            SecretUsePurpose::Git => "git",
        }
    }

    /// Read a stored `purpose` back.
    ///
    /// The column has no `CHECK`, so a row written by an older or newer
    /// version could hold anything; this is the one place that decides what
    /// the orchestrator accepts.
    pub fn parse(raw: &str) -> SecretResult<Self> {
        match raw {
            "launch" => Ok(SecretUsePurpose::Launch),
            "git" => Ok(SecretUsePurpose::Git),
            _ => Err(SecretError::InvalidPurpose),
        }
    }
}

impl std::fmt::Display for SecretUsePurpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `secret_uses` row, column for column (`docs/data-model.md`,
/// `secret_uses`).
///
/// `GET /secrets/{id}/uses` returns `{ session_id, user_id, purpose, at }`
/// (`SPEC.md`, "Secrets"); the route projects that from this row. Both ids are
/// optional and both being absent is normal — the mirror-fetch job sets
/// neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretUse {
    pub id: i64,
    pub secret_id: Uuid,
    pub session_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub purpose: SecretUsePurpose,
    pub at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::agent_profile::MAX_SECRET_NAME_CHARS;

    /// Not key material: obviously fake bytes wherever a test needs the
    /// encrypted half of a row (`CLAUDE.md`, rule 3).
    fn fake_value() -> EncryptedValue {
        EncryptedValue {
            ciphertext: b"fake-ciphertext".to_vec(),
            nonce: b"fake-nonce12".to_vec(),
            data_key_wrapped: b"fake-wrapped-data-key".to_vec(),
            data_key_nonce: b"fake-wrap-no".to_vec(),
            key_version: 1,
        }
    }

    #[test]
    fn every_error_is_a_bad_request_with_a_message() {
        for error in [
            SecretError::InvalidName,
            SecretError::InvalidScope,
            SecretError::InvalidPurpose,
            SecretError::InvalidValue,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }
    }

    #[test]
    fn the_crate_error_delegates_to_the_model() {
        let error = Error::from(SecretError::InvalidName);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), SecretError::InvalidName.to_string());
    }

    #[test]
    fn a_name_accepts_the_documented_pattern() {
        for raw in [
            "A",
            "GIT_CREDENTIAL",
            "X1",
            "A_",
            "ANTHROPIC_API_KEY",
            "Z9_9",
        ] {
            assert_eq!(
                SecretName::parse(raw).map(|name| name.as_str().to_string()),
                Ok(raw.to_string()),
                "{raw} should be accepted"
            );
        }
    }

    #[test]
    fn a_name_accepts_exactly_a_hundred_and_twenty_eight_characters() {
        let longest = format!("A{}", "B".repeat(MAX_SECRET_NAME_CHARS - 1));
        assert_eq!(longest.chars().count(), MAX_SECRET_NAME_CHARS);
        assert_eq!(SecretName::parse(&longest).unwrap().as_str(), longest);

        let too_long = format!("A{}", "B".repeat(MAX_SECRET_NAME_CHARS));
        assert_eq!(SecretName::parse(&too_long), Err(SecretError::InvalidName));
    }

    #[test]
    fn a_name_rejects_anything_outside_the_pattern() {
        for raw in [
            "",               // empty
            "a",              // lowercase
            "token",          // lowercase word
            "GIT_credential", // mixed case
            "1TOKEN",         // leading digit
            "_TOKEN",         // leading underscore
            " TOKEN",         // leading space
            "TOKEN ",         // trailing space
            "GIT-CRED",       // hyphen is not in the pattern
            "GIT.CRED",
            "GIT CRED",
            "TÖKEN", // non-ASCII
        ] {
            assert_eq!(
                SecretName::parse(raw),
                Err(SecretError::InvalidName),
                "{raw:?} should be rejected"
            );
        }
    }

    #[test]
    fn a_name_is_never_normalised() {
        // The name is an environment variable and part of the AAD, so the only
        // two outcomes are "stored verbatim" and "rejected".
        assert_eq!(SecretName::parse("TOKEN").unwrap().to_string(), "TOKEN");
        assert_eq!(String::from(SecretName::parse("TOKEN").unwrap()), "TOKEN");
        assert!(SecretName::parse(" TOKEN ").is_err());
    }

    #[test]
    fn a_value_is_accepted_between_one_byte_and_the_limit() {
        assert_eq!(validate_secret_value("x"), Ok(()));
        assert_eq!(validate_secret_value(" leading space"), Ok(()));
        assert_eq!(
            validate_secret_value(&"x".repeat(MAX_SECRET_VALUE_BYTES)),
            Ok(())
        );

        assert_eq!(validate_secret_value(""), Err(SecretError::InvalidValue));
        assert_eq!(
            validate_secret_value(&"x".repeat(MAX_SECRET_VALUE_BYTES + 1)),
            Err(SecretError::InvalidValue)
        );
    }

    #[test]
    fn a_value_is_measured_in_bytes_not_characters() {
        // Two bytes each, so half as many characters reach the limit.
        let at_the_limit = "ö".repeat(MAX_SECRET_VALUE_BYTES / 2);
        assert_eq!(at_the_limit.len(), MAX_SECRET_VALUE_BYTES);
        assert_eq!(validate_secret_value(&at_the_limit), Ok(()));

        let over = format!("{at_the_limit}ö");
        assert_eq!(over.chars().count(), MAX_SECRET_VALUE_BYTES / 2 + 1);
        assert_eq!(validate_secret_value(&over), Err(SecretError::InvalidValue));
    }

    #[test]
    fn the_value_rejection_names_the_limit_and_nothing_else() {
        let message = SecretError::InvalidValue.to_string();
        assert!(message.contains("65536"), "{message}");
    }

    #[test]
    fn a_scope_needs_an_id_exactly_when_it_is_not_global() {
        let id = Uuid::new_v4();

        assert_eq!(
            ScopeRef::new(SecretScope::Global, None).unwrap(),
            ScopeRef::global()
        );
        assert_eq!(
            ScopeRef::new(SecretScope::User, Some(id)),
            Ok(ScopeRef::user(id))
        );
        assert_eq!(
            ScopeRef::new(SecretScope::Project, Some(id)),
            Ok(ScopeRef::project(id))
        );

        assert_eq!(
            ScopeRef::new(SecretScope::Global, Some(id)),
            Err(SecretError::InvalidScope)
        );
        assert_eq!(
            ScopeRef::new(SecretScope::User, None),
            Err(SecretError::InvalidScope)
        );
        assert_eq!(
            ScopeRef::new(SecretScope::Project, None),
            Err(SecretError::InvalidScope)
        );
    }

    #[test]
    fn a_scope_reports_its_halves() {
        let id = Uuid::new_v4();

        assert_eq!(ScopeRef::global().scope(), SecretScope::Global);
        assert_eq!(ScopeRef::global().scope_id(), None);
        assert_eq!(ScopeRef::user(id).scope(), SecretScope::User);
        assert_eq!(ScopeRef::user(id).scope_id(), Some(id));
        assert_eq!(ScopeRef::project(id).scope(), SecretScope::Project);
        assert_eq!(ScopeRef::project(id).scope_id(), Some(id));
    }

    #[test]
    fn the_scopes_serialise_as_the_enum_values() {
        assert_eq!(SecretScope::Global.as_str(), "global");
        assert_eq!(SecretScope::User.as_str(), "user");
        assert_eq!(SecretScope::Project.as_str(), "project");
        assert_eq!(SecretScope::Project.to_string(), "project");
        assert_eq!(
            serde_json::to_value(SecretScope::Project).unwrap(),
            serde_json::json!("project")
        );
    }

    #[test]
    fn the_purposes_serialise_as_launch_and_git() {
        assert_eq!(SecretUsePurpose::Launch.as_str(), "launch");
        assert_eq!(SecretUsePurpose::Git.as_str(), "git");
        assert_eq!(SecretUsePurpose::Git.to_string(), "git");
        assert_eq!(
            serde_json::to_value(SecretUsePurpose::Launch).unwrap(),
            serde_json::json!("launch")
        );
        assert_eq!(
            serde_json::to_value(SecretUsePurpose::Git).unwrap(),
            serde_json::json!("git")
        );
        assert_eq!(
            serde_json::from_value::<SecretUsePurpose>(serde_json::json!("launch")).unwrap(),
            SecretUsePurpose::Launch
        );
    }

    #[test]
    fn a_purpose_round_trips_through_its_stored_spelling() {
        for purpose in [SecretUsePurpose::Launch, SecretUsePurpose::Git] {
            assert_eq!(SecretUsePurpose::parse(purpose.as_str()), Ok(purpose));
        }
        for raw in ["", "Launch", "LAUNCH", "fetch", "git "] {
            assert_eq!(
                SecretUsePurpose::parse(raw),
                Err(SecretError::InvalidPurpose),
                "{raw:?} should be rejected"
            );
        }
    }

    #[test]
    fn a_new_secret_starts_with_the_documented_defaults() {
        let scope = ScopeRef::global();
        let name = SecretName::parse("GIT_CREDENTIAL").unwrap();
        let secret = NewSecret::new(scope, name, fake_value());

        assert_eq!(secret.scope, scope);
        assert_eq!(secret.name.as_str(), "GIT_CREDENTIAL");
        assert_eq!(secret.value.key_version, 1);
        assert!(!secret.orchestrator_only);
        assert_eq!(secret.created_by, None);
    }

    fn fake_row() -> Secret {
        Secret {
            id: Uuid::nil(),
            scope: SecretScope::User,
            scope_id: Some(Uuid::nil()),
            name: "GIT_CREDENTIAL".into(),
            ciphertext: b"fake-ciphertext".to_vec(),
            nonce: b"fake-nonce12".to_vec(),
            data_key_wrapped: b"fake-wrapped-data-key".to_vec(),
            data_key_nonce: b"fake-wrap-no".to_vec(),
            key_version: 3,
            orchestrator_only: true,
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn a_row_never_serialises_its_encrypted_columns() {
        let json = serde_json::to_value(fake_row()).unwrap();

        for column in ["ciphertext", "nonce", "data_key_wrapped", "data_key_nonce"] {
            assert!(json.get(column).is_none(), "{column} leaked: {json}");
        }
        let rendered = json.to_string();
        assert!(!rendered.contains("fake-ciphertext"), "leaked: {rendered}");

        assert_eq!(json["name"], "GIT_CREDENTIAL");
        assert_eq!(json["scope"], "user");
        assert_eq!(json["key_version"], 3);
        assert_eq!(json["orchestrator_only"], true);
    }

    #[test]
    fn a_row_debug_redacts_its_encrypted_columns() {
        let rendered = format!("{:?}", fake_row());

        for bytes in [
            "fake-ciphertext",
            "fake-nonce12",
            "fake-wrapped-data-key",
            "fake-wrap-no",
        ] {
            assert!(!rendered.contains(bytes), "{bytes} leaked: {rendered}");
        }
        assert!(rendered.contains("<redacted>"), "{rendered}");
        // The identifying half is what a log line is allowed to carry.
        assert!(rendered.contains("GIT_CREDENTIAL"), "{rendered}");

        // And when the row is nested inside something else's `Debug`.
        let rendered = format!("{:?}", Some(("launch", fake_row())));
        assert!(!rendered.contains("fake-ciphertext"), "leaked: {rendered}");
    }

    #[test]
    fn an_encrypted_value_debug_redacts_everything_but_the_key_version() {
        let rendered = format!("{:?}", fake_value());

        for bytes in [
            "fake-ciphertext",
            "fake-nonce12",
            "fake-wrapped-data-key",
            "fake-wrap-no",
        ] {
            assert!(!rendered.contains(bytes), "{bytes} leaked: {rendered}");
        }
        assert!(rendered.contains("key_version: 1"), "{rendered}");

        // `NewSecret` derives `Debug` and holds one, so it inherits this.
        let secret = NewSecret::new(
            ScopeRef::global(),
            SecretName::parse("TOKEN").unwrap(),
            fake_value(),
        );
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("fake-ciphertext"), "leaked: {rendered}");
    }

    #[test]
    fn a_row_reports_its_scope_pair() {
        let row = fake_row();
        assert_eq!(row.scope_ref(), ScopeRef::user(Uuid::nil()));
    }

    #[test]
    fn a_row_hands_over_exactly_its_five_encrypted_columns() {
        let row = fake_row();
        let value = row.encrypted_value();

        assert_eq!(value.ciphertext, row.ciphertext);
        assert_eq!(value.nonce, row.nonce);
        assert_eq!(value.data_key_wrapped, row.data_key_wrapped);
        assert_eq!(value.data_key_nonce, row.data_key_nonce);
        assert_eq!(value.key_version, row.key_version);

        // A copy, not a borrow: the row is untouched and mutating the envelope
        // cannot reach back into it.
        let mut value = value;
        value.ciphertext.clear();
        assert_eq!(row.ciphertext, b"fake-ciphertext".to_vec());
    }
}
