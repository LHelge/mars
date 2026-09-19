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

use crate::models::SecretName;
// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// The only permission mode v1 accepts (`SPEC.md`, "Agent profiles").
pub const PERMISSION_MODE_BYPASS: &str = "bypass";

/// The column default for `idle_timeout_secs`, repeated here so a profile
/// built in code matches one built by the database.
pub const DEFAULT_IDLE_TIMEOUT_SECS: i32 = 1800;

/// Longest accepted profile name, in characters.
pub const MAX_PROFILE_NAME_CHARS: usize = 64;

/// Longest accepted `model`, in characters. The CLI's own model names are far
/// shorter; the cap only keeps a paste out of the column.
pub const MAX_MODEL_CHARS: usize = 100;

/// Longest accepted image reference, in characters — the length a registry
/// reference can reach, tag or digest included.
pub const MAX_IMAGE_CHARS: usize = 255;

/// Longest accepted `system_prompt`, in bytes. A system prompt is appended to
/// every launch of the profile, so 64 KiB is generous and still bounded.
pub const MAX_SYSTEM_PROMPT_BYTES: usize = 64 * 1024;

/// The state a profile serves when the caller names none (`SPEC.md`, "Agent
/// profiles": `serves_states` "default to `[\"ready\"]`").
pub const DEFAULT_SERVED_STATE: &str = "ready";

/// Every MCP tool name a profile may list in `mcp_tools`, in the order
/// `SPEC.md`, "MCP tool contracts" documents them: the task-tracker tools
/// first, then the four profile-gated git tools.
///
/// The task tools are served to every session whether they are listed or not;
/// listing gates only the git tools (`ARCHITECTURE.md`, "MCP design" → "Tool
/// exposure"). The list is still the whole known set, because it is what a
/// profile is validated against, and **the MCP epic reuses this constant** for
/// its `tools/list` gate rather than repeating the names: one list, one place
/// to change when a tool is added.
pub const KNOWN_MCP_TOOLS: &[&str] = &[
    "ready",
    "claim",
    "get_task",
    "update",
    "release",
    "comment",
    "needs_human",
    "create_task",
    "list_session_branches",
    "merge",
    "rebase",
    "push",
];

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
    /// The name was empty after trimming, or longer than
    /// [`MAX_PROFILE_NAME_CHARS`].
    #[error("profile name must be 1-64 characters")]
    InvalidName,
    /// The permission mode was something other than `bypass`.
    #[error("permission mode must be bypass")]
    UnsupportedPermissionMode,
    /// `model` was given but empty after trimming, or longer than
    /// [`MAX_MODEL_CHARS`].
    #[error("model must be 1-100 characters when set")]
    InvalidModel,
    /// The system prompt was longer than [`MAX_SYSTEM_PROMPT_BYTES`].
    #[error("system prompt must be at most 65536 bytes")]
    SystemPromptTooLong,
    /// The image reference was empty after trimming, or longer than
    /// [`MAX_IMAGE_CHARS`].
    #[error("profile image must be 1-255 characters")]
    InvalidImage,
    /// `runtime` was given but empty after trimming.
    #[error("runtime must not be empty when set")]
    InvalidRuntime,
    /// A listed MCP tool is not one of [`KNOWN_MCP_TOOLS`].
    #[error("unknown MCP tool \"{0}\"")]
    UnknownMcpTool(String),
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

/// An `agent_profiles` row, column for column (`docs/data-model.md`), plus the
/// `profile_states` link it is always read with.
///
/// This is the API-facing `Profile` of `SPEC.md`, "Agent profiles": every
/// column and `serves_states`, the names of the `task_states` rows the profile
/// serves in board order. The link table is a join away and every reader wants
/// it, so the repository's reads carry it rather than making each route
/// assemble the two.
///
/// There is no `Deserialize`: a profile row only ever comes out of the
/// database, and the caller-supplied shape is [`ProfileInput`].
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
    /// The names of the project's `queue` states this profile serves, in board
    /// order. Not a column: the `profile_states` link table.
    pub serves_states: Vec<String>,
    pub partial_messages: bool,
    pub idle_timeout_secs: i32,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentProfile {
    /// The `secrets` column as the validated names a launch resolves.
    ///
    /// The resolver is handed [`SecretName`]s rather than strings, so the
    /// pattern is checked on the way *into* the column
    /// ([`validate_secrets`]) and nowhere again on the way out
    /// (`ARCHITECTURE.md`, "Secrets", Resolution at launch). An entry that is
    /// not a name therefore cannot exist; one written by an older version, or
    /// straight into the column, is dropped with a warning rather than
    /// failing a launch, because a profile that lists an impossible name has
    /// no row to resolve it to either.
    pub fn secret_names(&self) -> Vec<SecretName> {
        self.secrets
            .iter()
            .filter_map(|raw| match SecretName::parse(raw) {
                Ok(name) => Some(name),
                Err(_) => {
                    warn!(
                        profile_id = %self.id,
                        secret_name = %raw,
                        "a stored profile secret is not a valid secret name"
                    );
                    None
                }
            })
            .collect()
    }
}

/// The caller-supplied half of a new agent profile.
///
/// Every column except the timestamps, because a profile has no field the
/// database invents for it, plus `serves_states`: the names the caller asked
/// the profile to serve, which are not a column but a `profile_states` write
/// the same transaction makes through
/// [`crate::repositories::TaskRepository::set_profile_states_by_name`].
///
/// `partial_messages` is the one exception to that symmetry: `None` means "use
/// the kind's default", and [`NewAgentProfile::validate`] resolves it **in
/// place**, so a struct that has been validated carries the value that will be
/// stored. [`NewAgentProfile::partial_messages`] resolves it either way, which
/// is what the repository binds.
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
    pub serves_states: Vec<String>,
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
            serves_states: vec![DEFAULT_SERVED_STATE.to_string()],
            partial_messages: None,
            idle_timeout_secs: DEFAULT_IDLE_TIMEOUT_SECS,
            is_default: false,
        };
        profile.validate()?;

        Ok(profile)
    }

    /// Check every rule in `SPEC.md`, "Agent profiles", trimming the text
    /// fields, dropping duplicate list entries and resolving
    /// `partial_messages` in place.
    ///
    /// Mutating rather than returning a new value keeps the caller's other
    /// fields — which are public and may have been set individually — without
    /// a rebuild, and makes it impossible to validate a profile and then
    /// insert the unvalidated one.
    pub fn validate(&mut self) -> ProfileResult<()> {
        self.name = validate_name(&self.name)?;
        self.model = validate_model(self.model.as_deref())?;
        validate_system_prompt(self.system_prompt.as_deref())?;
        self.image = validate_image(&self.image)?;
        self.runtime = validate_runtime(self.runtime.as_deref())?;
        validate_permission_mode(&self.permission_mode)?;
        validate_idle_timeout(self.idle_timeout_secs)?;
        self.mcp_tools = validate_mcp_tools(&self.mcp_tools)?;
        self.secrets = validate_secrets(&self.secrets)?;
        self.serves_states = deduplicate(&self.serves_states);
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

/// A validated profile with every default applied: what
/// [`ProfileInput::resolve`] yields and what the repository writes without
/// defaulting anything further.
///
/// A full replacement rather than a patch, because `ProfileInput` is the whole
/// profile minus the ids and timestamps: `PUT` with no `model` clears the
/// model, which a `COALESCE`-per-column update could not express. Two fields
/// are not columns. `serves_states` is the `profile_states` link table, written
/// by the tracker's repository in the same transaction. `is_default` is
/// `Option` because it is the one field a `PUT` may leave alone: `None` keeps
/// whatever the row has, and only an explicit `true` moves the flag
/// (`SPEC.md`, "Agent profiles").
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
    pub serves_states: Vec<String>,
    pub partial_messages: Option<bool>,
    pub idle_timeout_secs: i32,
    pub is_default: Option<bool>,
}

impl ProfileUpdate {
    /// The same rules as [`NewAgentProfile::validate`], applied in place.
    pub fn validate(&mut self) -> ProfileResult<()> {
        self.name = validate_name(&self.name)?;
        self.model = validate_model(self.model.as_deref())?;
        validate_system_prompt(self.system_prompt.as_deref())?;
        self.image = validate_image(&self.image)?;
        self.runtime = validate_runtime(self.runtime.as_deref())?;
        validate_permission_mode(&self.permission_mode)?;
        validate_idle_timeout(self.idle_timeout_secs)?;
        self.mcp_tools = validate_mcp_tools(&self.mcp_tools)?;
        self.secrets = validate_secrets(&self.secrets)?;
        self.serves_states = deduplicate(&self.serves_states);
        self.partial_messages = Some(self.partial_messages());

        Ok(())
    }

    /// Whether the updated profile streams partial messages, resolving `None`
    /// from the kind.
    pub fn partial_messages(&self) -> bool {
        self.partial_messages
            .unwrap_or_else(|| self.kind.default_partial_messages())
    }

    /// The same values as a brand new profile of `project_id`, with a fresh id.
    ///
    /// The insert path of the very same resolved values: `POST` and `PUT` share
    /// one validation, and only this decides which shape the repository is
    /// handed. An unset `is_default` is `false` on a create, which is the
    /// documented default — the project's one default profile is the seeded
    /// one until someone says otherwise.
    pub fn into_new(self, project_id: Uuid) -> NewAgentProfile {
        NewAgentProfile {
            id: Uuid::new_v4(),
            project_id,
            name: self.name,
            kind: self.kind,
            backend: self.backend,
            model: self.model,
            system_prompt: self.system_prompt,
            permission_mode: self.permission_mode,
            image: self.image,
            runtime: self.runtime,
            mcp_tools: self.mcp_tools,
            secrets: self.secrets,
            serves_states: self.serves_states,
            partial_messages: self.partial_messages,
            idle_timeout_secs: self.idle_timeout_secs,
            is_default: self.is_default.unwrap_or(false),
        }
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
            serves_states: profile.serves_states.clone(),
            partial_messages: Some(profile.partial_messages),
            idle_timeout_secs: profile.idle_timeout_secs,
            is_default: Some(profile.is_default),
        }
    }
}

/// The `ProfileInput` body of `POST` and `PUT /projects/{pid}/profiles`
/// (`SPEC.md`, "Agent profiles").
///
/// The whole profile minus the ids and timestamps, with everything but the
/// name optional. This is the only type that knows the *request* defaults —
/// `conversational`, `claude`, `bypass`, the configured session image,
/// `["ready"]`, 1800 seconds — and [`ProfileInput::resolve`] is the one place
/// they are applied, so no repository, route or test has to reproduce them.
///
/// Deserialising is deliberately lenient about which keys are present and
/// strict about their values: a missing key takes the default, a present one
/// is validated, and `null` is the same as missing for every nullable field.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct ProfileInput {
    pub name: String,
    #[serde(default)]
    pub kind: Option<ProfileKind>,
    #[serde(default)]
    pub backend: Option<AgentBackend>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub mcp_tools: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub serves_states: Option<Vec<String>>,
    #[serde(default)]
    pub partial_messages: Option<bool>,
    #[serde(default)]
    pub idle_timeout_secs: Option<i32>,
    #[serde(default)]
    pub is_default: Option<bool>,
}

impl ProfileInput {
    /// Apply every documented default and every documented rule, yielding the
    /// shape the repository stores.
    ///
    /// `image` falls back to `SESSION_IMAGE_DEFAULT` (`README.md`,
    /// "Configuration"), which is why this needs the configuration at all;
    /// every other default is a constant of this module. `partial_messages` is
    /// resolved from the kind, so the returned value is the one that will be
    /// stored, and `is_default` stays `Option` — see [`ProfileUpdate`].
    pub fn resolve(self, config: &Config) -> ProfileResult<ProfileUpdate> {
        let mut resolved = ProfileUpdate {
            name: self.name,
            kind: self.kind.unwrap_or(ProfileKind::Conversational),
            backend: self.backend.unwrap_or(AgentBackend::Claude),
            model: self.model,
            system_prompt: self.system_prompt,
            permission_mode: self
                .permission_mode
                .unwrap_or_else(|| PERMISSION_MODE_BYPASS.to_string()),
            image: self
                .image
                .unwrap_or_else(|| config.session_image_default.clone()),
            runtime: self.runtime,
            mcp_tools: self.mcp_tools,
            secrets: self.secrets,
            serves_states: self
                .serves_states
                .unwrap_or_else(|| vec![DEFAULT_SERVED_STATE.to_string()]),
            partial_messages: self.partial_messages,
            idle_timeout_secs: self.idle_timeout_secs.unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS),
            is_default: self.is_default,
        };
        resolved.validate()?;

        Ok(resolved)
    }

    /// [`ProfileInput::resolve`] as the insert shape for a new profile of
    /// `project_id`.
    pub fn resolve_new(self, project_id: Uuid, config: &Config) -> ProfileResult<NewAgentProfile> {
        Ok(self.resolve(config)?.into_new(project_id))
    }
}

/// The trimmed name at 1–[`MAX_PROFILE_NAME_CHARS`] characters, or
/// [`ProfileError::InvalidName`].
///
/// A profile name is a label in the launch menu, not an identifier, so the
/// only rules are that there is something to show and that it fits on a line.
fn validate_name(raw: &str) -> ProfileResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_PROFILE_NAME_CHARS {
        return Err(ProfileError::InvalidName);
    }
    Ok(trimmed.to_string())
}

/// The trimmed model name, or [`ProfileError::InvalidModel`].
///
/// `None` stays `None` — the CLI's own default — but a key that is present and
/// blank is a mistake, not a request for the default, because it would reach
/// the CLI as `--model ''`.
fn validate_model(raw: Option<&str>) -> ProfileResult<Option<String>> {
    let Some(trimmed) = raw.map(str::trim) else {
        return Ok(None);
    };
    if trimmed.is_empty() || trimmed.chars().count() > MAX_MODEL_CHARS {
        return Err(ProfileError::InvalidModel);
    }
    Ok(Some(trimmed.to_string()))
}

/// At most [`MAX_SYSTEM_PROMPT_BYTES`]; the text itself is untouched.
///
/// Deliberately not trimmed: a system prompt is prose the user wrote, and its
/// leading and trailing whitespace is theirs to keep.
fn validate_system_prompt(raw: Option<&str>) -> ProfileResult<()> {
    match raw {
        Some(prompt) if prompt.len() > MAX_SYSTEM_PROMPT_BYTES => {
            Err(ProfileError::SystemPromptTooLong)
        }
        _ => Ok(()),
    }
}

/// The trimmed image reference at 1–[`MAX_IMAGE_CHARS`] characters, or
/// [`ProfileError::InvalidImage`].
///
/// Only emptiness and length are checked: the reference is the engine's to
/// resolve, and second-guessing its grammar here would reject tags and digests
/// the engine accepts.
fn validate_image(raw: &str) -> ProfileResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_IMAGE_CHARS {
        return Err(ProfileError::InvalidImage);
    }
    Ok(trimmed.to_string())
}

/// The trimmed runtime name, or [`ProfileError::InvalidRuntime`].
///
/// `None` means the engine default; a blank string would be passed as
/// `HostConfig.Runtime` and fail the launch instead.
fn validate_runtime(raw: Option<&str>) -> ProfileResult<Option<String>> {
    let Some(trimmed) = raw.map(str::trim) else {
        return Ok(None);
    };
    if trimmed.is_empty() {
        return Err(ProfileError::InvalidRuntime);
    }
    Ok(Some(trimmed.to_string()))
}

/// The listed tools, deduplicated, once every one is a [`KNOWN_MCP_TOOLS`]
/// name.
///
/// A typo here is silent at runtime — the profile would simply never be served
/// the tool it asked for — so it is rejected at the door, naming the entry
/// (`SPEC.md`, "Agent profiles": "`mcp_tools` entries must be known tool
/// names").
fn validate_mcp_tools(tools: &[String]) -> ProfileResult<Vec<String>> {
    for tool in tools {
        if !KNOWN_MCP_TOOLS.contains(&tool.as_str()) {
            return Err(ProfileError::UnknownMcpTool(tool.clone()));
        }
    }

    Ok(deduplicate(tools))
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

/// The listed secrets, deduplicated, once every entry is an
/// environment-variable name.
///
/// The pattern itself is [`SecretName`]'s and is not repeated here: a
/// profile's `secrets` are the names of `secrets` rows, so there is one rule
/// and one place it lives (`docs/data-model.md`, `secrets`). Only the error
/// changes, because a malformed entry in a profile body is a profile's
/// rejection.
fn validate_secrets(secrets: &[String]) -> ProfileResult<Vec<String>> {
    for name in secrets {
        SecretName::parse(name).map_err(|_| ProfileError::InvalidSecretName)?;
    }

    Ok(deduplicate(secrets))
}

/// `values` without repeats, keeping the caller's order.
///
/// `mcp_tools`, `secrets` and `serves_states` are sets the caller sent as
/// lists. Order is kept rather than sorted because it is the order the user
/// typed and the order the UI will show back; `profile_states` has a primary
/// key that would reject a repeat outright, and the two `TEXT[]` columns would
/// simply store one.
fn deduplicate(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::with_capacity(values.len());

    values
        .iter()
        .filter(|value| seen.insert(value.as_str()))
        .cloned()
        .collect()
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

    /// A stored row, for the accessors that only a read has.
    fn profile_row() -> AgentProfile {
        AgentProfile {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            name: "default".into(),
            kind: ProfileKind::Conversational,
            backend: AgentBackend::Claude,
            model: None,
            system_prompt: None,
            permission_mode: PERMISSION_MODE_BYPASS.into(),
            image: TEST_IMAGE.into(),
            runtime: None,
            mcp_tools: Vec::new(),
            secrets: Vec::new(),
            serves_states: vec![DEFAULT_SERVED_STATE.into()],
            partial_messages: true,
            idle_timeout_secs: DEFAULT_IDLE_TIMEOUT_SECS,
            is_default: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn every_error_is_a_bad_request_with_a_message() {
        for error in [
            ProfileError::InvalidName,
            ProfileError::UnsupportedPermissionMode,
            ProfileError::InvalidModel,
            ProfileError::SystemPromptTooLong,
            ProfileError::InvalidImage,
            ProfileError::InvalidRuntime,
            ProfileError::UnknownMcpTool("nope".into()),
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
        assert_eq!(profile.serves_states, [DEFAULT_SERVED_STATE]);
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
    fn every_secret_entry_is_checked_against_the_one_name_rule() {
        // The pattern itself is `SecretName::parse`'s and is asserted in
        // `models::secret`; what this file owns is that a profile applies it
        // to every entry and reports its own error.
        let mut profile = profile();
        profile.secrets = vec!["ANTHROPIC_API_KEY".to_string(), "NPM_TOKEN".to_string()];
        assert!(profile.validate().is_ok());

        for raw in ["not-a-name", "lower", "9LEADING", ""] {
            profile.secrets = vec!["NPM_TOKEN".to_string(), raw.to_string()];
            assert_eq!(
                profile.validate(),
                Err(ProfileError::InvalidSecretName),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn a_row_hands_the_resolver_names_and_drops_what_cannot_be_one() {
        let mut row = profile_row();
        row.secrets = vec!["NPM_TOKEN".into(), "ANTHROPIC_API_KEY".into()];

        assert_eq!(
            row.secret_names()
                .iter()
                .map(SecretName::as_str)
                .collect::<Vec<_>>(),
            ["NPM_TOKEN", "ANTHROPIC_API_KEY"]
        );

        // Validation refuses these on the way in, so this is the defence in
        // depth for a column written by something else: a name no row can
        // carry is dropped rather than passed on.
        row.secrets = vec!["npm_token".into(), "NPM TOKEN".into(), "NPM_TOKEN".into()];
        assert_eq!(
            row.secret_names()
                .iter()
                .map(SecretName::as_str)
                .collect::<Vec<_>>(),
            ["NPM_TOKEN"]
        );
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
            serves_states: vec!["review".into()],
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
        assert_eq!(update.serves_states, row.serves_states);
        assert_eq!(update.partial_messages(), row.partial_messages);
        assert_eq!(update.is_default, Some(row.is_default));

        // And the same values as a fresh profile of another project.
        let new = update.clone().into_new(Uuid::nil());
        assert_ne!(new.id, row.id);
        assert_eq!(new.project_id, Uuid::nil());
        assert_eq!(new.name, row.name);
        assert_eq!(new.serves_states, row.serves_states);
        assert!(new.is_default);

        update.permission_mode = "plan".into();
        assert_eq!(
            update.validate(),
            Err(ProfileError::UnsupportedPermissionMode)
        );
    }

    /// Obviously fake values; nothing here is a real credential (rule 3).
    fn test_config() -> Config {
        let vars: std::collections::HashMap<&str, &str> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "unix:///run/user/1000/podman/podman.sock"),
            ("DATA_DIR_HOST", "/srv/mars/data"),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
            ("SESSION_IMAGE_DEFAULT", TEST_IMAGE),
        ]
        .into_iter()
        .collect();

        Config::from_vars(|name| vars.get(name).map(|value| value.to_string()))
            .expect("a complete required set loads")
    }

    /// The smallest body `POST /projects/{pid}/profiles` accepts.
    fn input(name: &str) -> ProfileInput {
        ProfileInput {
            name: name.to_string(),
            ..ProfileInput::default()
        }
    }

    #[test]
    fn an_input_takes_every_documented_default() {
        let resolved = input("planner").resolve(&test_config()).unwrap();

        assert_eq!(resolved.name, "planner");
        assert_eq!(resolved.kind, ProfileKind::Conversational);
        assert_eq!(resolved.backend, AgentBackend::Claude);
        assert_eq!(resolved.model, None);
        assert_eq!(resolved.system_prompt, None);
        assert_eq!(resolved.permission_mode, PERMISSION_MODE_BYPASS);
        // `SESSION_IMAGE_DEFAULT` (`README.md`, "Configuration").
        assert_eq!(resolved.image, TEST_IMAGE);
        assert_eq!(resolved.runtime, None);
        assert!(resolved.mcp_tools.is_empty());
        assert!(resolved.secrets.is_empty());
        assert_eq!(resolved.serves_states, ["ready"]);
        assert_eq!(resolved.partial_messages, Some(true));
        assert_eq!(resolved.idle_timeout_secs, DEFAULT_IDLE_TIMEOUT_SECS);
        // Unset, so the row keeps whatever it has; a create reads it as false.
        assert_eq!(resolved.is_default, None);
        assert!(
            !input("planner")
                .resolve_new(Uuid::nil(), &test_config())
                .unwrap()
                .is_default
        );
    }

    #[test]
    fn an_input_deserialises_from_the_documented_body() {
        let input: ProfileInput = serde_json::from_value(serde_json::json!({
            "name": "reviewer",
            "kind": "ephemeral",
            "backend": "claude",
            "model": null,
            "mcp_tools": ["merge", "push"],
            "secrets": ["NPM_TOKEN"],
            "serves_states": ["review"],
            "idle_timeout_secs": 60,
            "is_default": true,
        }))
        .expect("the body deserialises");

        let resolved = input.resolve(&test_config()).unwrap();
        assert_eq!(resolved.kind, ProfileKind::Ephemeral);
        assert_eq!(resolved.mcp_tools, ["merge", "push"]);
        assert_eq!(resolved.secrets, ["NPM_TOKEN"]);
        assert_eq!(resolved.serves_states, ["review"]);
        assert_eq!(resolved.idle_timeout_secs, 60);
        assert_eq!(resolved.is_default, Some(true));
        // Ephemeral, and nothing was said, so streaming is off.
        assert_eq!(resolved.partial_messages, Some(false));
    }

    #[test]
    fn partial_messages_defaults_per_kind_and_survives_an_explicit_value() {
        let mut ephemeral = input("runner");
        ephemeral.kind = Some(ProfileKind::Ephemeral);
        assert_eq!(
            ephemeral
                .clone()
                .resolve(&test_config())
                .unwrap()
                .partial_messages,
            Some(false)
        );

        ephemeral.partial_messages = Some(true);
        assert_eq!(
            ephemeral.resolve(&test_config()).unwrap().partial_messages,
            Some(true)
        );

        let mut conversational = input("pair");
        conversational.partial_messages = Some(false);
        assert_eq!(
            conversational
                .resolve(&test_config())
                .unwrap()
                .partial_messages,
            Some(false)
        );
    }

    #[test]
    fn an_empty_serves_states_is_kept_and_is_not_the_default() {
        let mut serves_nothing = input("hand-launched");
        serves_nothing.serves_states = Some(Vec::new());
        assert!(
            serves_nothing
                .resolve(&test_config())
                .unwrap()
                .serves_states
                .is_empty()
        );
    }

    #[test]
    fn the_text_fields_are_bounded() {
        let config = test_config();

        let mut long_name = input(&"n".repeat(MAX_PROFILE_NAME_CHARS));
        assert!(long_name.clone().resolve(&config).is_ok());
        long_name.name = "n".repeat(MAX_PROFILE_NAME_CHARS + 1);
        assert_eq!(long_name.resolve(&config), Err(ProfileError::InvalidName));

        let mut model = input("planner");
        model.model = Some(format!("  {} ", "m".repeat(MAX_MODEL_CHARS)));
        assert_eq!(
            model.clone().resolve(&config).unwrap().model,
            Some("m".repeat(MAX_MODEL_CHARS))
        );
        model.model = Some("m".repeat(MAX_MODEL_CHARS + 1));
        assert_eq!(
            model.clone().resolve(&config),
            Err(ProfileError::InvalidModel)
        );
        model.model = Some("   ".to_string());
        assert_eq!(model.resolve(&config), Err(ProfileError::InvalidModel));

        let mut prompt = input("planner");
        prompt.system_prompt = Some("p".repeat(MAX_SYSTEM_PROMPT_BYTES));
        assert!(prompt.clone().resolve(&config).is_ok());
        prompt.system_prompt = Some("p".repeat(MAX_SYSTEM_PROMPT_BYTES + 1));
        assert_eq!(
            prompt.resolve(&config),
            Err(ProfileError::SystemPromptTooLong)
        );

        let mut image = input("planner");
        image.image = Some("i".repeat(MAX_IMAGE_CHARS + 1));
        assert_eq!(
            image.clone().resolve(&config),
            Err(ProfileError::InvalidImage)
        );
        image.image = Some("  ".to_string());
        assert_eq!(image.resolve(&config), Err(ProfileError::InvalidImage));

        let mut runtime = input("planner");
        runtime.runtime = Some(" runsc ".to_string());
        assert_eq!(
            runtime.clone().resolve(&config).unwrap().runtime.as_deref(),
            Some("runsc")
        );
        runtime.runtime = Some(String::new());
        assert_eq!(runtime.resolve(&config), Err(ProfileError::InvalidRuntime));
    }

    #[test]
    fn an_idle_timeout_of_less_than_a_second_is_refused() {
        let config = test_config();
        let mut input = input("planner");
        for secs in [i32::MIN, -1, 0] {
            input.idle_timeout_secs = Some(secs);
            assert_eq!(
                input.clone().resolve(&config),
                Err(ProfileError::InvalidIdleTimeout),
                "accepted {secs}"
            );
        }
        input.idle_timeout_secs = Some(1);
        assert_eq!(input.resolve(&config).unwrap().idle_timeout_secs, 1);
    }

    #[test]
    fn every_mcp_tool_must_be_a_known_one() {
        let config = test_config();

        let mut every_tool = input("planner");
        every_tool.mcp_tools = KNOWN_MCP_TOOLS
            .iter()
            .map(|tool| tool.to_string())
            .collect();
        assert_eq!(
            every_tool.resolve(&config).unwrap().mcp_tools.len(),
            KNOWN_MCP_TOOLS.len()
        );
        // The twelve of `SPEC.md`, "MCP tool contracts", and no more.
        assert_eq!(KNOWN_MCP_TOOLS.len(), 12);
        for tool in ["ready", "claim", "get_task", "create_task", "merge", "push"] {
            assert!(KNOWN_MCP_TOOLS.contains(&tool), "{tool} is not known");
        }

        let mut unknown = input("planner");
        unknown.mcp_tools = vec!["ready".into(), "sudo".into()];
        assert_eq!(
            unknown.clone().resolve(&config),
            Err(ProfileError::UnknownMcpTool("sudo".into()))
        );
        assert_eq!(
            Error::from(ProfileError::UnknownMcpTool("sudo".into())).to_string(),
            "unknown MCP tool \"sudo\""
        );
    }

    #[test]
    fn repeated_list_entries_are_dropped_in_order() {
        let mut input = input("planner");
        input.mcp_tools = vec!["merge".into(), "ready".into(), "merge".into()];
        input.secrets = vec!["NPM_TOKEN".into(), "NPM_TOKEN".into()];
        input.serves_states = Some(vec!["review".into(), "ready".into(), "review".into()]);

        let resolved = input.resolve(&test_config()).unwrap();
        assert_eq!(resolved.mcp_tools, ["merge", "ready"]);
        assert_eq!(resolved.secrets, ["NPM_TOKEN"]);
        assert_eq!(resolved.serves_states, ["review", "ready"]);
    }

    #[test]
    fn a_secret_name_is_an_environment_variable_name_here_too() {
        let mut input = input("planner");
        input.secrets = vec!["ANTHROPIC_API_KEY".into(), "npm_token".into()];
        assert_eq!(
            input.resolve(&test_config()),
            Err(ProfileError::InvalidSecretName)
        );
    }

    #[test]
    fn an_input_permission_mode_other_than_bypass_is_refused() {
        let config = test_config();
        let mut input = input("planner");
        for raw in ["acceptEdits", "plan", "Bypass", "bypass ", ""] {
            input.permission_mode = Some(raw.to_string());
            assert_eq!(
                input.clone().resolve(&config),
                Err(ProfileError::UnsupportedPermissionMode),
                "accepted {raw:?}"
            );
        }
        input.permission_mode = Some(PERMISSION_MODE_BYPASS.to_string());
        assert!(input.resolve(&config).is_ok());
    }
}
