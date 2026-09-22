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

use crate::models::{CronSchedule, ScheduleError, SecretName};
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

/// Longest accepted `schedule_prompt`, in bytes. It is the `message` of every
/// scheduled launch, so the same bound as a system prompt is generous and
/// still bounded.
pub const MAX_SCHEDULE_PROMPT_BYTES: usize = 64 * 1024;

/// The column default for `max_concurrent`, repeated here for the reason
/// [`DEFAULT_IDLE_TIMEOUT_SECS`] is.
pub const DEFAULT_MAX_CONCURRENT: i32 = 1;

/// Fewest live sessions an unattended launch of a profile may be allowed
/// (`ARCHITECTURE.md`, "Task tracker" → "Unattended launches").
///
/// A cap of zero would be "never launch", which `auto_launch = false` already
/// says; the table `CHECK` is the same bound, so this is what keeps a zero from
/// reaching the database as a 500 instead of a 400.
pub const MIN_MAX_CONCURRENT: i32 = 1;

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

impl AgentBackend {
    /// The enum value's exact spelling, which is the one the column, the JSON
    /// and any message naming a backend use (`docs/data-model.md`, "Enums").
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentBackend::Claude => "claude",
        }
    }
}

impl std::fmt::Display for AgentBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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
    /// A listed secret is some backend's agent credential, which the launcher
    /// resolves without the profile declaring it (ADR 0036).
    #[error("{0} is an agent credential and is injected automatically")]
    AgentCredentialSecret(String),
    /// `auto_launch` was set on a `conversational` profile, which exists to
    /// talk to a person (ADR 0042).
    #[error("auto_launch requires an ephemeral profile")]
    AutoLaunchNotEphemeral,
    /// `max_concurrent` was below [`MIN_MAX_CONCURRENT`].
    #[error("max_concurrent must be at least 1")]
    InvalidMaxConcurrent,
    /// `schedule_cron` was not a 5-field UTC cron expression (ADR 0043).
    #[error(transparent)]
    InvalidScheduleCron(#[from] ScheduleError),
    /// A schedule was set on a profile that is not `ephemeral`, which a
    /// scheduled launch may not be (ADR 0042, ADR 0043).
    #[error("schedule_cron requires an ephemeral profile")]
    ScheduleNotEphemeral,
    /// `schedule_cron` was set without the prompt that run is given.
    #[error("schedule_prompt is required when schedule_cron is set")]
    ScheduleWithoutPrompt,
    /// `schedule_prompt` was set on a profile with no schedule to use it.
    #[error("schedule_prompt requires schedule_cron")]
    SchedulePromptWithoutSchedule,
    /// The schedule prompt was longer than [`MAX_SCHEDULE_PROMPT_BYTES`].
    #[error("schedule prompt must be at most 65536 bytes")]
    SchedulePromptTooLong,
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
    /// Whether the dispatcher may start a session of this profile by itself
    /// (`ARCHITECTURE.md`, "Task tracker" → "Unattended launches"; ADR 0042).
    pub auto_launch: bool,
    /// How many live sessions of this profile an unattended launch may leave
    /// behind. Read by the scheduler too, whether or not `auto_launch` is set.
    pub max_concurrent: i32,
    /// The 5-field UTC cron expression this profile is launched on, or `None`
    /// for a profile nothing schedules (`ARCHITECTURE.md`, "Task tracker" →
    /// "Scheduled agents"; ADR 0043).
    pub schedule_cron: Option<String>,
    /// What a scheduled run is told to do: the `message` of the launch. Set
    /// exactly when [`AgentProfile::schedule_cron`] is.
    pub schedule_prompt: Option<String>,
    /// When the scheduler last decided a tick of this profile fires. Written
    /// by the job in the transaction that decides it, read-only over REST, and
    /// cleared when the schedule is.
    pub last_scheduled_at: Option<DateTime<Utc>>,
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
    ///
    /// An agent credential name is dropped the same way and for the same
    /// reason: [`validate_secrets`] refuses one on the way in and a migration
    /// removed the ones older rows carried, so this is the defence in depth
    /// for a column written by something else. The launcher resolves the
    /// credential itself (ADR 0036), and letting a listed one through would
    /// only make the declared half of the resolution compete with it.
    /// The stored `schedule_cron` as the parsed expression the scheduler and
    /// the API read it through, or `None` when there is no schedule.
    ///
    /// A stored expression that does not parse is dropped with a warning
    /// rather than failing the read, for the reason [`secret_names`] drops an
    /// impossible name: [`validate_schedule`] refuses one on the way in, so a
    /// column that holds one was written by something else, and a profile that
    /// cannot be shown is worse than one whose next run is unknown.
    ///
    /// [`secret_names`]: AgentProfile::secret_names
    pub fn schedule(&self) -> Option<CronSchedule> {
        let raw = self.schedule_cron.as_deref()?;

        match CronSchedule::parse(raw) {
            Ok(schedule) => Some(schedule),
            Err(error) => {
                warn!(
                    profile_id = %self.id,
                    %error,
                    "a stored schedule expression does not parse"
                );
                None
            }
        }
    }

    /// The next instant this profile is due to launch, strictly after `after`.
    ///
    /// `None` without a schedule, and `None` for a stored expression that does
    /// not parse or can never fire — which is what `SPEC.md`, "Agent profiles"
    /// documents `next_scheduled_at` as. It is computed rather than stored so
    /// that an expression and its next run cannot disagree, and it is answered
    /// here so no frontend needs a cron parser (ADR 0043).
    pub fn next_scheduled_at(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.schedule()?.next_after(after)
    }

    pub fn secret_names(&self) -> Vec<SecretName> {
        self.secrets
            .iter()
            .filter_map(|raw| match SecretName::parse(raw) {
                Ok(_) if crate::agent::credential_backend_of(raw).is_some() => {
                    warn!(
                        profile_id = %self.id,
                        secret_name = %raw,
                        "a stored profile secret is an agent credential and is injected anyway"
                    );
                    None
                }
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
    pub auto_launch: bool,
    pub max_concurrent: i32,
    pub schedule_cron: Option<String>,
    pub schedule_prompt: Option<String>,
    /// When this profile was created, for the one caller that cannot let the
    /// column default decide: project creation.
    ///
    /// `created_at` defaults to `now()`, which in Postgres is the
    /// *transaction's* start, so several profiles inserted by one transaction
    /// share it to the microsecond and `ORDER BY created_at` — the documented
    /// "oldest first" of `GET /projects/{pid}/profiles` — would fall through
    /// to the name. The four seeded role profiles are listed in the order a
    /// task travels through them, not alphabetically, so
    /// [`crate::projects::create_project`] spaces their timestamps by hand
    /// (`SPEC.md`, "Role profile templates"). Every other caller leaves this
    /// `None` and takes the column default.
    pub created_at: Option<DateTime<Utc>>,
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
            auto_launch: false,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            schedule_cron: None,
            schedule_prompt: None,
            created_at: None,
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
        validate_unattended(self.kind, self.auto_launch, self.max_concurrent)?;
        (self.schedule_cron, self.schedule_prompt) = validate_schedule(
            self.kind,
            self.schedule_cron.as_deref(),
            self.schedule_prompt.as_deref(),
        )?;

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
    pub auto_launch: bool,
    pub max_concurrent: i32,
    pub schedule_cron: Option<String>,
    pub schedule_prompt: Option<String>,
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
        validate_unattended(self.kind, self.auto_launch, self.max_concurrent)?;
        (self.schedule_cron, self.schedule_prompt) = validate_schedule(
            self.kind,
            self.schedule_cron.as_deref(),
            self.schedule_prompt.as_deref(),
        )?;

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
            auto_launch: self.auto_launch,
            max_concurrent: self.max_concurrent,
            schedule_cron: self.schedule_cron,
            schedule_prompt: self.schedule_prompt,
            created_at: None,
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
            auto_launch: profile.auto_launch,
            max_concurrent: profile.max_concurrent,
            schedule_cron: profile.schedule_cron.clone(),
            schedule_prompt: profile.schedule_prompt.clone(),
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
    #[serde(default)]
    pub auto_launch: Option<bool>,
    #[serde(default)]
    pub max_concurrent: Option<i32>,
    #[serde(default)]
    pub schedule_cron: Option<String>,
    #[serde(default)]
    pub schedule_prompt: Option<String>,
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
            auto_launch: self.auto_launch.unwrap_or(false),
            max_concurrent: self.max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT),
            schedule_cron: self.schedule_cron,
            schedule_prompt: self.schedule_prompt,
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
///
/// An entry that is some backend's agent credential is refused outright: the
/// launcher resolves the credential for every session of its backend without
/// the profile declaring it, so listing one is redundant and misleading
/// (ADR 0036; `ARCHITECTURE.md`, "Secrets", Agent credentials). The names are
/// the adapters' — [`crate::agent::credential_backend_of`] — and not repeated
/// here, and every backend's count, not just the profile's own: a Claude
/// profile has no use for another backend's credential either.
fn validate_secrets(secrets: &[String]) -> ProfileResult<Vec<String>> {
    for name in secrets {
        SecretName::parse(name).map_err(|_| ProfileError::InvalidSecretName)?;
        if crate::agent::credential_backend_of(name).is_some() {
            return Err(ProfileError::AgentCredentialSecret(name.clone()));
        }
    }

    Ok(deduplicate(secrets))
}

/// The two unattended-launch rules of `ARCHITECTURE.md`, "Task tracker" →
/// "Unattended launches" that a profile can decide by itself.
///
/// `auto_launch` is for `ephemeral` profiles only: a conversational profile
/// exists to talk to a person, and starting one unattended produces a session
/// waiting for input nobody asked for (ADR 0042). `max_concurrent` is valid on
/// any profile whether or not `auto_launch` is set, because the scheduler reads
/// it too, and is at least [`MIN_MAX_CONCURRENT`].
///
/// The third rule — the backend's agent credential has to resolve without a
/// user — is not here: it is a question for the `secrets` table, so it lives
/// beside the lookup that answers it
/// ([`crate::secrets::require_unattended_credential`]) and a model that holds
/// no SQL cannot ask it (`CLAUDE.md`, "Backend conventions").
fn validate_unattended(
    kind: ProfileKind,
    auto_launch: bool,
    max_concurrent: i32,
) -> ProfileResult<()> {
    if auto_launch && kind != ProfileKind::Ephemeral {
        return Err(ProfileError::AutoLaunchNotEphemeral);
    }
    if max_concurrent < MIN_MAX_CONCURRENT {
        return Err(ProfileError::InvalidMaxConcurrent);
    }

    Ok(())
}

/// The schedule rules of `ARCHITECTURE.md`, "Task tracker" → "Scheduled
/// agents" that a profile can decide by itself, yielding the pair the columns
/// store.
///
/// The two fields stand or fall together, which is the table's own `CHECK`:
/// an expression with no prompt is a scheduled agent that was never told what
/// to do, and a prompt with no expression is a setting nothing will ever read.
/// A prompt that is blank or only whitespace is no prompt, so it is refused
/// beside an expression and simply cleared without one — a form that sends an
/// empty box for a schedule it is not setting is clearing the schedule, not
/// making a mistake.
///
/// The expression itself is [`CronSchedule`]'s to judge (ADR 0043), the kind
/// rule is [`validate_unattended`]'s reason in this section's words — only an
/// `ephemeral` profile may be launched with nobody behind it — and the third
/// rule, the backend's agent credential, is the `secrets` table's and lives
/// with the lookup that answers it, exactly as it does for `auto_launch`.
///
/// The prompt keeps its own whitespace, as `system_prompt` does: it is prose
/// the user wrote.
fn validate_schedule(
    kind: ProfileKind,
    cron: Option<&str>,
    prompt: Option<&str>,
) -> ProfileResult<(Option<String>, Option<String>)> {
    let prompt = prompt.filter(|text| !text.trim().is_empty());

    let Some(raw) = cron.map(str::trim).filter(|raw| !raw.is_empty()) else {
        if prompt.is_some() {
            return Err(ProfileError::SchedulePromptWithoutSchedule);
        }
        return Ok((None, None));
    };

    let schedule = CronSchedule::parse(raw)?;
    let prompt = prompt.ok_or(ProfileError::ScheduleWithoutPrompt)?;
    if prompt.len() > MAX_SCHEDULE_PROMPT_BYTES {
        return Err(ProfileError::SchedulePromptTooLong);
    }
    if kind != ProfileKind::Ephemeral {
        return Err(ProfileError::ScheduleNotEphemeral);
    }

    Ok((
        Some(schedule.as_str().to_string()),
        Some(prompt.to_string()),
    ))
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
    use chrono::TimeZone;

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
            auto_launch: false,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            schedule_cron: None,
            schedule_prompt: None,
            last_scheduled_at: None,
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
            ProfileError::AgentCredentialSecret("ANTHROPIC_API_KEY".into()),
            ProfileError::AutoLaunchNotEphemeral,
            ProfileError::InvalidMaxConcurrent,
            ProfileError::InvalidScheduleCron(ScheduleError::Shape),
            ProfileError::InvalidScheduleCron(ScheduleError::Invalid("nope".into())),
            ProfileError::ScheduleNotEphemeral,
            ProfileError::ScheduleWithoutPrompt,
            ProfileError::SchedulePromptWithoutSchedule,
            ProfileError::SchedulePromptTooLong,
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
        // Nothing launches itself until somebody says so (ADR 0042).
        assert!(!profile.auto_launch);
        assert_eq!(profile.max_concurrent, DEFAULT_MAX_CONCURRENT);
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
        profile.secrets = vec!["DEPLOY_TOKEN".to_string(), "NPM_TOKEN".to_string()];
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
    fn an_agent_credential_name_is_refused_in_a_profiles_secrets() {
        // Every backend's names, not just this profile's backend (ADR 0036);
        // the names themselves are the adapters' and asserted in `agent`.
        let mut profile = profile();
        for raw in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"] {
            profile.secrets = vec!["NPM_TOKEN".to_string(), raw.to_string()];
            assert_eq!(
                profile.validate(),
                Err(ProfileError::AgentCredentialSecret(raw.to_string())),
                "accepted {raw}"
            );
        }

        // The message names the first offending entry in input order.
        profile.secrets = vec![
            "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
            "ANTHROPIC_API_KEY".to_string(),
        ];
        assert_eq!(
            profile.validate().unwrap_err().to_string(),
            "CLAUDE_CODE_OAUTH_TOKEN is an agent credential and is injected automatically"
        );
    }

    #[test]
    fn a_row_hands_the_resolver_names_and_drops_what_cannot_be_one() {
        let mut row = profile_row();
        row.secrets = vec!["NPM_TOKEN".into(), "DEPLOY_TOKEN".into()];

        assert_eq!(
            row.secret_names()
                .iter()
                .map(SecretName::as_str)
                .collect::<Vec<_>>(),
            ["NPM_TOKEN", "DEPLOY_TOKEN"]
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
    fn a_row_drops_an_agent_credential_name_too() {
        // The same defence in depth: validation refuses one on the way in and
        // a migration removed the ones older rows carried, so a credential
        // here is a column written by something else. The launcher injects it
        // anyway (ADR 0036), so the declared half drops it.
        let mut row = profile_row();
        row.secrets = vec![
            "NPM_TOKEN".into(),
            "CLAUDE_CODE_OAUTH_TOKEN".into(),
            "ANTHROPIC_API_KEY".into(),
            "DEPLOY_TOKEN".into(),
        ];

        assert_eq!(
            row.secret_names()
                .iter()
                .map(SecretName::as_str)
                .collect::<Vec<_>>(),
            ["NPM_TOKEN", "DEPLOY_TOKEN"]
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
            auto_launch: true,
            max_concurrent: 3,
            schedule_cron: Some("30 3 * * *".into()),
            schedule_prompt: Some("scan for tech debt".into()),
            last_scheduled_at: Some(Utc::now()),
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
        assert_eq!(update.auto_launch, row.auto_launch);
        assert_eq!(update.max_concurrent, row.max_concurrent);
        assert_eq!(update.schedule_cron, row.schedule_cron);
        assert_eq!(update.schedule_prompt, row.schedule_prompt);

        // And the same values as a fresh profile of another project.
        let new = update.clone().into_new(Uuid::nil());
        assert_eq!(new.auto_launch, row.auto_launch);
        assert_eq!(new.max_concurrent, row.max_concurrent);
        assert_eq!(new.schedule_cron, row.schedule_cron);
        assert_eq!(new.schedule_prompt, row.schedule_prompt);
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
        input.secrets = vec!["DEPLOY_TOKEN".into(), "npm_token".into()];
        assert_eq!(
            input.resolve(&test_config()),
            Err(ProfileError::InvalidSecretName)
        );
    }

    #[test]
    fn an_agent_credential_name_is_refused_from_an_input_too() {
        let mut input = input("planner");
        input.secrets = vec!["NPM_TOKEN".into(), "ANTHROPIC_API_KEY".into()];
        assert_eq!(
            input.resolve(&test_config()),
            Err(ProfileError::AgentCredentialSecret(
                "ANTHROPIC_API_KEY".into()
            ))
        );
    }

    #[test]
    fn auto_launch_is_refused_on_a_conversational_profile() {
        // The rule of `ARCHITECTURE.md`, "Task tracker" → "Unattended
        // launches": an unattended launch produces an ephemeral session, and a
        // conversational one started by itself waits for input nobody asked
        // for.
        let mut profile = profile();
        profile.auto_launch = true;
        assert_eq!(
            profile.validate(),
            Err(ProfileError::AutoLaunchNotEphemeral)
        );

        profile.kind = ProfileKind::Ephemeral;
        assert!(profile.validate().is_ok());

        // And `false` is legal on either kind.
        profile.auto_launch = false;
        profile.kind = ProfileKind::Conversational;
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn max_concurrent_is_at_least_one_on_any_profile() {
        let mut profile = profile();
        for cap in [i32::MIN, -1, 0] {
            profile.max_concurrent = cap;
            assert_eq!(
                profile.validate(),
                Err(ProfileError::InvalidMaxConcurrent),
                "accepted {cap}"
            );
        }

        // Valid on an ephemeral profile that does not launch itself: the
        // scheduler reads it too.
        profile.kind = ProfileKind::Ephemeral;
        profile.auto_launch = false;
        profile.max_concurrent = MIN_MAX_CONCURRENT;
        assert!(profile.validate().is_ok());
        profile.max_concurrent = 5;
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn an_input_takes_the_unattended_defaults_and_carries_an_explicit_pair() {
        let config = test_config();

        let resolved = input("planner").resolve(&config).unwrap();
        assert!(!resolved.auto_launch);
        assert_eq!(resolved.max_concurrent, DEFAULT_MAX_CONCURRENT);

        let mut scanner = input("scanner");
        scanner.kind = Some(ProfileKind::Ephemeral);
        scanner.auto_launch = Some(true);
        scanner.max_concurrent = Some(3);
        let resolved = scanner.clone().resolve(&config).unwrap();
        assert!(resolved.auto_launch);
        assert_eq!(resolved.max_concurrent, 3);
        // And the insert shape carries them on unchanged.
        let new = scanner.resolve_new(Uuid::nil(), &config).unwrap();
        assert!(new.auto_launch);
        assert_eq!(new.max_concurrent, 3);
    }

    #[test]
    fn an_input_is_refused_for_the_same_two_reasons_a_profile_is() {
        let config = test_config();

        let mut conversational = input("planner");
        conversational.auto_launch = Some(true);
        assert_eq!(
            conversational.resolve(&config),
            Err(ProfileError::AutoLaunchNotEphemeral)
        );

        let mut capped = input("planner");
        capped.max_concurrent = Some(0);
        assert_eq!(
            capped.resolve(&config),
            Err(ProfileError::InvalidMaxConcurrent)
        );
    }

    // ---- schedules (`ARCHITECTURE.md`, "Scheduled agents"; ADR 0043) ----

    /// An ephemeral profile with the schedule `cron` and the prompt `prompt`.
    fn scheduled(cron: Option<&str>, prompt: Option<&str>) -> NewAgentProfile {
        let mut profile = profile();
        profile.kind = ProfileKind::Ephemeral;
        profile.schedule_cron = cron.map(str::to_string);
        profile.schedule_prompt = prompt.map(str::to_string);
        profile
    }

    #[test]
    fn a_five_field_expression_with_a_prompt_is_a_schedule() {
        let mut profile = scheduled(Some("  30 3 * * 1-5 "), Some("scan for tech debt"));
        assert!(profile.validate().is_ok());

        // Stored trimmed, and the prompt exactly as it was written.
        assert_eq!(profile.schedule_cron.as_deref(), Some("30 3 * * 1-5"));
        assert_eq!(
            profile.schedule_prompt.as_deref(),
            Some("scan for tech debt")
        );
    }

    #[test]
    fn an_expression_that_is_not_five_plain_fields_is_refused() {
        // A seconds field, a year field and an `@`-form are all things
        // `croner` itself would take; the one accepted dialect is 5 fields.
        for cron in ["0 30 3 * * *", "30 3 * * * 2026", "@daily", "* * * *"] {
            let mut profile = scheduled(Some(cron), Some("scan"));
            assert_eq!(
                profile.validate(),
                Err(ProfileError::InvalidScheduleCron(ScheduleError::Shape)),
                "accepted {cron:?}"
            );
        }

        // Five fields of nonsense are the parser's own complaint.
        let mut profile = scheduled(Some("nope not a cron here"), Some("scan"));
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::InvalidScheduleCron(ScheduleError::Invalid(_)))
        ));
    }

    #[test]
    fn a_schedule_without_a_prompt_is_refused() {
        for prompt in [None, Some(""), Some("   \n ")] {
            let mut profile = scheduled(Some("30 3 * * *"), prompt);
            assert_eq!(
                profile.validate(),
                Err(ProfileError::ScheduleWithoutPrompt),
                "accepted {prompt:?}"
            );
        }

        let long = "x".repeat(MAX_SCHEDULE_PROMPT_BYTES + 1);
        let mut profile = scheduled(Some("30 3 * * *"), Some(&long));
        assert_eq!(profile.validate(), Err(ProfileError::SchedulePromptTooLong));
    }

    #[test]
    fn a_prompt_without_a_schedule_is_refused_and_a_blank_one_is_cleared() {
        let mut profile = scheduled(None, Some("scan for tech debt"));
        assert_eq!(
            profile.validate(),
            Err(ProfileError::SchedulePromptWithoutSchedule)
        );

        // The two columns stand or fall together, so a body that carries an
        // empty box and no expression is clearing the schedule.
        let mut profile = scheduled(None, Some("  "));
        assert!(profile.validate().is_ok());
        assert_eq!(profile.schedule_cron, None);
        assert_eq!(profile.schedule_prompt, None);
    }

    #[test]
    fn a_schedule_is_refused_on_a_conversational_profile() {
        let mut profile = scheduled(Some("30 3 * * *"), Some("scan"));
        profile.kind = ProfileKind::Conversational;
        assert_eq!(profile.validate(), Err(ProfileError::ScheduleNotEphemeral));

        profile.kind = ProfileKind::Ephemeral;
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn an_input_carries_a_schedule_and_clears_it_by_omission() {
        let config = test_config();

        let mut scanner = input("scanner");
        scanner.kind = Some(ProfileKind::Ephemeral);
        scanner.schedule_cron = Some("30 3 * * *".into());
        scanner.schedule_prompt = Some("scan for tech debt".into());
        let resolved = scanner.clone().resolve(&config).unwrap();
        assert_eq!(resolved.schedule_cron.as_deref(), Some("30 3 * * *"));
        assert_eq!(
            resolved.schedule_prompt.as_deref(),
            Some("scan for tech debt")
        );

        let new = scanner.resolve_new(Uuid::nil(), &config).unwrap();
        assert_eq!(new.schedule_cron.as_deref(), Some("30 3 * * *"));

        // `PUT` replaces the whole profile, so a body with neither field is a
        // profile with no schedule.
        let plain = input("scanner").resolve(&config).unwrap();
        assert_eq!(plain.schedule_cron, None);
        assert_eq!(plain.schedule_prompt, None);
    }

    #[test]
    fn a_rows_next_run_follows_its_stored_expression() {
        let mut row = profile_row();
        assert_eq!(row.next_scheduled_at(Utc::now()), None);

        row.schedule_cron = Some("30 3 * * *".into());
        row.schedule_prompt = Some("scan".into());
        let after = Utc.with_ymd_and_hms(2026, 9, 22, 4, 0, 0).unwrap();
        assert_eq!(
            row.next_scheduled_at(after),
            Some(Utc.with_ymd_and_hms(2026, 9, 23, 3, 30, 0).unwrap())
        );

        // A column written by something else is not an error to report at read
        // time: there is simply no next run to show.
        row.schedule_cron = Some("@daily".into());
        assert_eq!(row.next_scheduled_at(after), None);
        assert!(row.schedule().is_none());
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
