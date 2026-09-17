//! Agent profiles: the per-project configuration of one kind of agent.
//!
//! `docs/data-model.md`, `agent_profiles` is the column contract and
//! `SPEC.md`, "Agent profiles" the field contract. A profile is what a launch
//! reads to build a session: which image and runtime to start, which backend
//! adapter drives it, what to pass the CLI, which MCP tools and which secrets
//! it may see, and how long it may stay silent before the idle reaper acts
//! (`ARCHITECTURE.md`, "Task tracker").
//!
//! Two rules are worth stating here rather than in a table. `permission_mode`
//! is an adapter-specific string and v1 accepts only `bypass`, so the model
//! refuses anything else instead of letting a typo reach the CLI as a silently
//! different mode. And `partial_messages` has no column default on purpose:
//! the model fills it from the profile's kind — streaming on for a
//! conversational agent someone is watching, off for an ephemeral one that
//! reports once — so an insert that forgets it fails at the database rather
//! than guessing.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// The only permission mode v1 accepts (`SPEC.md`, "Agent profiles").
pub const PERMISSION_MODE_BYPASS: &str = "bypass";

/// Longest accepted secret name, in characters (`docs/data-model.md`,
/// `secrets`): one leading letter and up to 127 more characters.
pub const MAX_SECRET_NAME_CHARS: usize = 128;

/// The column default for `idle_timeout_secs`, repeated here so a profile
/// built in code matches one built by the database.
pub const DEFAULT_IDLE_TIMEOUT_SECS: i32 = 1800;

/// What a profile's sessions do (`docs/data-model.md`, "Enums").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "profile_kind", rename_all = "snake_case")]
pub enum ProfileKind {
    /// Takes input over stdin, is parked when idle and resumed later.
    Conversational,
    /// Runs one prompt and ends; never parked, resumed or retried.
    Ephemeral,
}

impl ProfileKind {
    /// Whether a session of this kind streams partial messages by default.
    ///
    /// A person watching a conversational session wants the text as it
    /// arrives; an ephemeral session reports once and the partial events would
    /// only be stored and never read (`docs/data-model.md`,
    /// `agent_profiles`).
    pub fn default_partial_messages(self) -> bool {
        matches!(self, ProfileKind::Conversational)
    }
}

/// Which CLI adapter drives sessions of a profile (`docs/data-model.md`,
/// "Enums").
///
/// One value in v1. A second backend is added as a new enum value by
/// migration, never by renaming this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "agent_backend", rename_all = "snake_case")]
pub enum AgentBackend {
    /// Claude Code, behind the `AgentBackend` trait.
    Claude,
}

/// Every way an agent-profile model can reject its input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// The name was empty after trimming.
    #[error("profile name must not be empty")]
    InvalidName,
    /// The permission mode was something other than `bypass`.
    #[error("permission mode must be bypass")]
    UnsupportedPermissionMode,
    /// The image reference was empty after trimming.
    #[error("profile image must not be empty")]
    InvalidImage,
    /// The idle timeout was below one second.
    #[error("idle timeout must be at least 1 second")]
    InvalidIdleTimeout,
    /// A secret name did not match `[A-Z][A-Z0-9_]*` at 1–128 characters.
    #[error("secret names must be 1-128 characters matching [A-Z][A-Z0-9_]*")]
    InvalidSecretName,
}

impl ProfileError {
    /// The HTTP status this rejection maps to.
    ///
    /// Every variant is malformed input, so every variant is 400. A name that
    /// is well formed but already used in the project, or a second default
    /// profile, is decided against the table's indexes and surfaces as
    /// [`Error::Conflict`] from the repository instead (`SPEC.md`, "Agent
    /// profiles").
    pub fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// The result type the agent-profile models return.
pub type ProfileResult<T> = std::result::Result<T, ProfileError>;

/// Does `raw` match the secret-name pattern, `[A-Z][A-Z0-9_]*` at 1–128
/// characters?
///
/// A profile's `secrets` are the names it wants injected as environment
/// variables, so the pattern is the environment-variable one:
/// `ANTHROPIC_API_KEY`, never `anthropic-api-key`. The secrets epic needs the
/// same rule for `secrets.name`, which is why this is a free function and not
/// a method — it moves to that module unchanged when it lands
/// (`docs/data-model.md`, `secrets`).
pub fn is_secret_name(raw: &str) -> bool {
    let mut characters = raw.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    if raw.chars().count() > MAX_SECRET_NAME_CHARS {
        return false;
    }
    characters.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// An `agent_profiles` row, column for column (`docs/data-model.md`).
///
/// The API-facing `Profile` (`SPEC.md`, "Agent profiles") is this row plus
/// `serves_states`, which is the `profile_states` link table and belongs to
/// the tracker's repository, so the routes assemble the two.
///
/// There is no `Deserialize`: a profile row only ever comes out of the
/// database, and the caller-supplied shape is [`NewAgentProfile`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct AgentProfile {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub kind: ProfileKind,
    pub backend: AgentBackend,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub permission_mode: String,
    pub image: String,
    pub runtime: Option<String>,
    pub mcp_tools: Vec<String>,
    pub secrets: Vec<String>,
    pub partial_messages: bool,
    pub idle_timeout_secs: i32,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The caller-supplied half of a new agent profile.
///
/// Every column except the timestamps, because a profile has no field the
/// database invents for it. `partial_messages` is the one exception to that
/// symmetry: `None` means "use the kind's default", and
/// [`NewAgentProfile::validate`] resolves it **in place**, so a struct that has
/// been validated carries the value that will be stored.
/// [`NewAgentProfile::partial_messages`] resolves it either way, which is what
/// the repository binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAgentProfile {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub kind: ProfileKind,
    pub backend: AgentBackend,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub permission_mode: String,
    pub image: String,
    pub runtime: Option<String>,
    pub mcp_tools: Vec<String>,
    pub secrets: Vec<String>,
    pub partial_messages: Option<bool>,
    pub idle_timeout_secs: i32,
    pub is_default: bool,
}

impl NewAgentProfile {
    /// A conversational Claude profile with a fresh id and the documented
    /// column defaults, validated.
    ///
    /// This is the shape of the `default` profile every project is created
    /// with (`docs/data-model.md`, `agent_profiles`); the remaining fields are
    /// public, and a caller that changes any of them calls
    /// [`NewAgentProfile::validate`] again.
    pub fn new(project_id: Uuid, name: &str, image: &str) -> ProfileResult<Self> {
        let mut profile = Self {
            id: Uuid::new_v4(),
            project_id,
            name: name.to_string(),
            kind: ProfileKind::Conversational,
            backend: AgentBackend::Claude,
            model: None,
            system_prompt: None,
            permission_mode: PERMISSION_MODE_BYPASS.to_string(),
            image: image.to_string(),
            runtime: None,
            mcp_tools: Vec::new(),
            secrets: Vec::new(),
            partial_messages: None,
            idle_timeout_secs: DEFAULT_IDLE_TIMEOUT_SECS,
            is_default: false,
        };
        profile.validate()?;

        Ok(profile)
    }

    /// Check every rule in `SPEC.md`, "Agent profiles", trimming `name` and
    /// `image` and resolving `partial_messages` in place.
    ///
    /// Mutating rather than returning a new value keeps the caller's other
    /// fields — which are public and may have been set individually — without
    /// a rebuild, and makes it impossible to validate a profile and then
    /// insert the unvalidated one.
    pub fn validate(&mut self) -> ProfileResult<()> {
        self.name = validate_name(&self.name)?;
        self.image = validate_image(&self.image)?;
        validate_permission_mode(&self.permission_mode)?;
        validate_idle_timeout(self.idle_timeout_secs)?;
        validate_secrets(&self.secrets)?;
        self.partial_messages = Some(self.partial_messages());

        Ok(())
    }

    /// Whether this profile streams partial messages, resolving `None` from
    /// the kind.
    pub fn partial_messages(&self) -> bool {
        self.partial_messages
            .unwrap_or_else(|| self.kind.default_partial_messages())
    }
}

/// The body `PUT /projects/{pid}/profiles/{id}` sends (`SPEC.md`, "Agent
/// profiles").
///
/// A full replacement rather than a patch, because `ProfileInput` is the whole
/// profile minus the ids and timestamps: `PUT` with no `model` clears the
/// model, which a `COALESCE`-per-column update could not express. `serves_states`
/// is not here — it is the `profile_states` link table, written by the
/// tracker's repository in the same transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileUpdate {
    pub name: String,
    pub kind: ProfileKind,
    pub backend: AgentBackend,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub permission_mode: String,
    pub image: String,
    pub runtime: Option<String>,
    pub mcp_tools: Vec<String>,
    pub secrets: Vec<String>,
    pub partial_messages: Option<bool>,
    pub idle_timeout_secs: i32,
    pub is_default: bool,
}

impl ProfileUpdate {
    /// The same rules as [`NewAgentProfile::validate`], applied in place.
    pub fn validate(&mut self) -> ProfileResult<()> {
        self.name = validate_name(&self.name)?;
        self.image = validate_image(&self.image)?;
        validate_permission_mode(&self.permission_mode)?;
        validate_idle_timeout(self.idle_timeout_secs)?;
        validate_secrets(&self.secrets)?;
        self.partial_messages = Some(self.partial_messages());

        Ok(())
    }

    /// Whether the updated profile streams partial messages, resolving `None`
    /// from the kind.
    pub fn partial_messages(&self) -> bool {
        self.partial_messages
            .unwrap_or_else(|| self.kind.default_partial_messages())
    }
}

impl From<&AgentProfile> for ProfileUpdate {
    /// The update that would leave `profile` exactly as it is, for callers
    /// that change one field of an existing row.
    fn from(profile: &AgentProfile) -> Self {
        Self {
            name: profile.name.clone(),
            kind: profile.kind,
            backend: profile.backend,
            model: profile.model.clone(),
            system_prompt: profile.system_prompt.clone(),
            permission_mode: profile.permission_mode.clone(),
            image: profile.image.clone(),
            runtime: profile.runtime.clone(),
            mcp_tools: profile.mcp_tools.clone(),
            secrets: profile.secrets.clone(),
            partial_messages: Some(profile.partial_messages),
            idle_timeout_secs: profile.idle_timeout_secs,
            is_default: profile.is_default,
        }
    }
}

/// The trimmed name, or [`ProfileError::InvalidName`] when nothing is left.
fn validate_name(raw: &str) -> ProfileResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProfileError::InvalidName);
    }
    Ok(trimmed.to_string())
}

/// The trimmed image reference, or [`ProfileError::InvalidImage`].
///
/// Only emptiness is checked: the reference is the engine's to resolve, and
/// second-guessing its grammar here would reject tags and digests the engine
/// accepts.
fn validate_image(raw: &str) -> ProfileResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProfileError::InvalidImage);
    }
    Ok(trimmed.to_string())
}

/// `bypass` and nothing else, in v1.
fn validate_permission_mode(raw: &str) -> ProfileResult<()> {
    if raw == PERMISSION_MODE_BYPASS {
        Ok(())
    } else {
        Err(ProfileError::UnsupportedPermissionMode)
    }
}

/// At least one second, so the idle reaper cannot be handed a timeout that
/// fires on every pass.
fn validate_idle_timeout(secs: i32) -> ProfileResult<()> {
    if secs >= 1 {
        Ok(())
    } else {
        Err(ProfileError::InvalidIdleTimeout)
    }
}

/// Every entry an environment-variable name.
fn validate_secrets(secrets: &[String]) -> ProfileResult<()> {
    if secrets.iter().all(|name| is_secret_name(name)) {
        Ok(())
    } else {
        Err(ProfileError::InvalidSecretName)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a real image: the stub the tests use everywhere (`CLAUDE.md`, rule
    /// 3).
    const TEST_IMAGE: &str = "mars-session-stub:test";

    fn profile() -> NewAgentProfile {
        NewAgentProfile::new(Uuid::nil(), "default", TEST_IMAGE).expect("the defaults are valid")
    }

    #[test]
    fn every_error_is_a_bad_request_with_a_message() {
        for error in [
            ProfileError::InvalidName,
            ProfileError::UnsupportedPermissionMode,
            ProfileError::InvalidImage,
            ProfileError::InvalidIdleTimeout,
            ProfileError::InvalidSecretName,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }
    }

    #[test]
    fn the_crate_error_delegates_to_the_model() {
        let error = Error::from(ProfileError::UnsupportedPermissionMode);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            error.to_string(),
            ProfileError::UnsupportedPermissionMode.to_string()
        );
    }

    #[test]
    fn the_enums_serialise_in_snake_case() {
        assert_eq!(
            serde_json::to_value(ProfileKind::Conversational).unwrap(),
            serde_json::json!("conversational")
        );
        assert_eq!(
            serde_json::from_value::<ProfileKind>(serde_json::json!("ephemeral")).unwrap(),
            ProfileKind::Ephemeral
        );
        assert_eq!(
            serde_json::to_value(AgentBackend::Claude).unwrap(),
            serde_json::json!("claude")
        );
    }

    #[test]
    fn a_new_profile_starts_with_the_documented_defaults() {
        let profile = profile();
        assert_eq!(profile.name, "default");
        assert_eq!(profile.kind, ProfileKind::Conversational);
        assert_eq!(profile.backend, AgentBackend::Claude);
        assert_eq!(profile.permission_mode, PERMISSION_MODE_BYPASS);
        assert_eq!(profile.image, TEST_IMAGE);
        assert_eq!(profile.idle_timeout_secs, DEFAULT_IDLE_TIMEOUT_SECS);
        assert!(profile.mcp_tools.is_empty());
        assert!(profile.secrets.is_empty());
        assert!(!profile.is_default);
        // Resolved in place by the `validate()` inside `new`.
        assert_eq!(profile.partial_messages, Some(true));
    }

    #[test]
    fn partial_messages_follows_the_kind_when_unset() {
        let mut profile = profile();
        profile.partial_messages = None;
        assert!(profile.partial_messages());
        profile.kind = ProfileKind::Ephemeral;
        assert!(!profile.partial_messages());

        // And resolving is what `validate` writes back.
        profile.validate().unwrap();
        assert_eq!(profile.partial_messages, Some(false));
        assert!(!ProfileKind::Ephemeral.default_partial_messages());
        assert!(ProfileKind::Conversational.default_partial_messages());
    }

    #[test]
    fn an_explicit_partial_messages_survives_validation() {
        let mut profile = profile();
        profile.kind = ProfileKind::Ephemeral;
        profile.partial_messages = Some(true);
        profile.validate().unwrap();
        assert_eq!(profile.partial_messages, Some(true));
        assert!(profile.partial_messages());
    }

    #[test]
    fn a_name_and_an_image_are_trimmed_and_non_empty() {
        let mut profile = profile();
        profile.name = "  planner \n".to_string();
        profile.image = format!("  {TEST_IMAGE} ");
        profile.validate().unwrap();
        assert_eq!(profile.name, "planner");
        assert_eq!(profile.image, TEST_IMAGE);

        for raw in ["", "   ", "\t\n"] {
            let mut blank_name = profile.clone();
            blank_name.name = raw.to_string();
            assert_eq!(blank_name.validate(), Err(ProfileError::InvalidName));

            let mut blank_image = profile.clone();
            blank_image.image = raw.to_string();
            assert_eq!(blank_image.validate(), Err(ProfileError::InvalidImage));
        }

        assert_eq!(
            NewAgentProfile::new(Uuid::nil(), " ", TEST_IMAGE).unwrap_err(),
            ProfileError::InvalidName
        );
        assert_eq!(
            NewAgentProfile::new(Uuid::nil(), "default", "").unwrap_err(),
            ProfileError::InvalidImage
        );
    }

    #[test]
    fn the_permission_mode_must_be_bypass() {
        let mut profile = profile();
        for raw in ["acceptEdits", "plan", "Bypass", "bypass ", "", "default"] {
            profile.permission_mode = raw.to_string();
            assert_eq!(
                profile.validate(),
                Err(ProfileError::UnsupportedPermissionMode),
                "accepted {raw:?}"
            );
        }
        profile.permission_mode = PERMISSION_MODE_BYPASS.to_string();
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn the_idle_timeout_must_be_at_least_one_second() {
        let mut profile = profile();
        for secs in [i32::MIN, -1, 0] {
            profile.idle_timeout_secs = secs;
            assert_eq!(
                profile.validate(),
                Err(ProfileError::InvalidIdleTimeout),
                "accepted {secs}"
            );
        }
        profile.idle_timeout_secs = 1;
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn secret_names_are_environment_variable_names() {
        for raw in ["A", "GIT_CREDENTIAL", "ANTHROPIC_API_KEY", "S3", "A_1_B"] {
            assert!(is_secret_name(raw), "rejected {raw:?}");
        }
        assert!(is_secret_name(&format!(
            "A{}",
            "B".repeat(MAX_SECRET_NAME_CHARS - 1)
        )));

        for raw in [
            "",
            "_LEADING",
            "9LEADING",
            "lower",
            "MIXEDcase",
            "WITH-DASH",
            "WITH SPACE",
            "WITH.DOT",
            "WITH$",
        ] {
            assert!(!is_secret_name(raw), "accepted {raw:?}");
        }
        assert!(!is_secret_name(&format!(
            "A{}",
            "B".repeat(MAX_SECRET_NAME_CHARS)
        )));
    }

    #[test]
    fn every_secret_entry_is_checked() {
        let mut profile = profile();
        profile.secrets = vec!["ANTHROPIC_API_KEY".to_string(), "NPM_TOKEN".to_string()];
        assert!(profile.validate().is_ok());

        profile.secrets.push("not-a-name".to_string());
        assert_eq!(profile.validate(), Err(ProfileError::InvalidSecretName));
    }

    #[test]
    fn an_update_round_trips_a_row_unchanged() {
        let row = AgentProfile {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            name: "planner".into(),
            kind: ProfileKind::Ephemeral,
            backend: AgentBackend::Claude,
            model: Some("a-model".into()),
            system_prompt: Some("plan carefully".into()),
            permission_mode: PERMISSION_MODE_BYPASS.into(),
            image: TEST_IMAGE.into(),
            runtime: Some("runsc".into()),
            mcp_tools: vec!["ready".into()],
            secrets: vec!["NPM_TOKEN".into()],
            partial_messages: false,
            idle_timeout_secs: 60,
            is_default: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let mut update = ProfileUpdate::from(&row);
        update.validate().unwrap();
        assert_eq!(update.name, row.name);
        assert_eq!(update.kind, row.kind);
        assert_eq!(update.model, row.model);
        assert_eq!(update.runtime, row.runtime);
        assert_eq!(update.partial_messages(), row.partial_messages);
        assert_eq!(update.is_default, row.is_default);

        update.permission_mode = "plan".into();
        assert_eq!(
            update.validate(),
            Err(ProfileError::UnsupportedPermissionMode)
        );
    }
}
