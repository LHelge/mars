//! Creating a session: the one path a launch takes, whoever asked for it.
//!
//! [`create_session`] is the whole of `POST /projects/{pid}/sessions` that is
//! not HTTP, and it is here rather than in [`crate::routes::sessions`] because
//! a launch is not a request: the dispatcher and the scheduled agents planned
//! in `ARCHITECTURE.md`, "Unattended launches", launch
//! "through the same path as a user launch", and that path must therefore need
//! neither an HTTP request nor a user. The route above it parses a body, calls
//! this and answers 201.
//!
//! **Who asked** is [`LaunchActor`]: a user, the dispatcher or a schedule. It
//! decides three things and nothing else — the `created_by` on the row (null
//! for the two unattended ones, which have no user to name), the
//! [`TaskActor`] the tracker mutation runs under (`system` for them), and the
//! `user_id` a queued caller message carries.
//!
//! **What is asked for** is [`LaunchRequest`]: the profile, an optional task,
//! an optional first message, an optional title and an optional `base_ref` —
//! the documented body of that endpoint, already parsed.
//!
//! **What it decides**, in this order, because the cheap refusals come first
//! and nothing irreversible happens before the last of them (`SPEC.md`,
//! "Sessions", `POST /projects/{pid}/sessions`; `ARCHITECTURE.md`, "Launch
//! sequence"):
//!
//! 1. the project exists (404) and is `ready` (409) — a project without a
//!    `default_branch` is answered with the same 409, because a session has no
//!    base to start from and a `ready` project always has one;
//! 2. the profile is one of *this* project's (400), which is also what makes
//!    `kind` the profile's kind at launch (`docs/data-model.md`, `sessions`);
//! 3. an ephemeral profile was given a prompt (400), since an ephemeral
//!    session accepts no input after launch (ADR 0003);
//! 4. an explicitly given `title` is 1 to 200 characters (400), and an omitted
//!    one is derived by [`default_title`];
//! 5. the base ref resolves in the project repository (400), checked under the
//!    project git lock and before any transaction, because a git lock is never
//!    taken from inside a database one (ADR 0021). The launcher resolves it
//!    again when it clones — the mirror may have been fetched in between — so
//!    this check exists only to answer 400 now instead of producing a session
//!    that fails a moment later. A base a task's hand-off pins is not checked
//!    here: it is chosen under the tracker lock this launch has not taken yet,
//!    and it is a commit the project retains;
//! 6. a fresh MCP token is generated **before** the insert, so the row's hash
//!    and the only copy of the value come into existence together (ADR 0029;
//!    `ARCHITECTURE.md`, "MCP design"): the raw token goes to the launcher,
//!    which writes it into the session's `mcp.json`, and never back to the UI.
//!
//! The row is then inserted and committed, the [`Session`] is answered, and the
//! launch runs on its own task.
//!
//! **Launching for a task** ([`insert_claiming`]) is the one create that writes
//! outside `sessions`: with a task the row insert and the claim are one
//! project-locked transaction, so a session that exists holds its task and a
//! claim that loses leaves no session behind (`ARCHITECTURE.md`, "Task
//! tracker" → "Launching a session for a task"). The base a hand-off pins, the
//! generated first message and the 409 for a task that is held, blocked or
//! finished all follow from the row read under that lock.
//!
//! **Served states are the caller's question.** A user launch ignores the
//! profile's served states — the user chose the pairing — and so does this
//! function unless [`LaunchRequest::require_served_state`] is set, which is how
//! an unattended launch says that the task must be in a state its profile
//! actually serves. The check reads `profile_states` under the same lock the
//! claim is decided under, as the MCP `claim` tool does, so a queue edited
//! meanwhile cannot let a launch through.
//!
//! **How the first message is delivered.** It is not a column: `message` is the
//! first thing said to this session — after the generated task message, when
//! there is a task — and where the two go depends on the kind. An ephemeral
//! session has no stdin to say them on, so they become the `-p` prompt the
//! launcher passes, joined by a blank line ([`LaunchMode::Fresh`]). A
//! conversational session reads its messages from stdin, so each is submitted
//! to the registry as a [`QueuedInput`] — the generated one with no `user_id`,
//! since nobody typed it, the caller's with the launching user's, or with none
//! when nobody launched it — and the owner records them as the session's first
//! `user_message`s exactly as it records any other input (`ARCHITECTURE.md`,
//! "Session owner task").
//!
//! That submission has to be race-free against the launch, which is why the
//! *caller* registers the session entry: [`SessionRegistry::submit`] rejects an
//! id it has never seen, and the launch task only registers once it starts. So
//! the order here is: commit the row, `register(.., Phase::Creating)`, submit
//! the input, launch. The launch task's own `register` keeps the queue of an id
//! it already knows — that is what makes a message to a parked session survive
//! the resume it triggers — so the queued input is picked up by the owner when
//! stdin attaches, whichever of the two `register` calls ran first.
//!
//! **The token hash never leaves.** The value answered is the [`Session`]
//! model, whose `mcp_token_hash` is `#[serde(skip)]`, and the raw token exists
//! only as the value moved into [`LaunchMode::Fresh`] (`CLAUDE.md`, rule 3).

use chrono::Utc;
use uuid::Uuid;

use super::launcher::LaunchMode;
use super::registry::{Phase, QueuedInput, SessionRegistry, SubmitResult};
use super::task_message::{HandoffContext, generated_task_message};
use super::token::McpToken;
use crate::events::{SessionInput, TaskActor};
use crate::git::{DataPaths, resolve_base};
use crate::models::{
    AgentProfile, NewSession, ProjectStatus, Session, SessionKind, TaskRef, default_title,
    validate_launch_prompt, validate_title,
};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use crate::tracker::{
    NOT_SERVED, TrackerMutation, claim_for_launch, retry_on_serialization_failure,
};

/// What a create against a project that has no repository to clone is told
/// (409).
///
/// The same words `routes::projects` answers a fetch or a branch listing with,
/// because it is the same condition: the project is not `ready`.
pub const NOT_READY: &str = "project is not ready";

/// What a create naming a profile of another project — or of no project — is
/// told (400).
///
/// One message for "no such profile" and "not this project's profile" alike:
/// the lookup is scoped in the `WHERE` clause (`CLAUDE.md`, "Backend
/// conventions"), so which of the two it was is not something a caller learns.
pub const UNKNOWN_PROFILE: &str = "unknown profile";

/// What a create whose base ref is not in the mirror is told (400).
pub const UNRESOLVED_BASE: &str = "base_ref does not resolve";

/// Who asked for this session.
///
/// Three variants because those are the three things that launch a session: a
/// person pressing a button, the dispatcher putting an agent on a claimable
/// task, or a schedule ticking (`ARCHITECTURE.md`, "Unattended
/// launches"). Only the first has a user to attribute the launch to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchActor {
    /// A signed-in user, through `POST /projects/{pid}/sessions`.
    User {
        /// The user the session is `created_by` and whose name the tracker
        /// event carries.
        user_id: Uuid,
    },
    /// The dispatcher, putting a profile on a claimable task by itself.
    Dispatcher,
    /// A scheduled agent, at a tick of its own cron expression.
    Schedule,
}

impl LaunchActor {
    /// The user to record as `sessions.created_by`, or `None` for a launch
    /// nobody asked for by hand.
    pub fn user_id(self) -> Option<Uuid> {
        match self {
            Self::User { user_id } => Some(user_id),
            Self::Dispatcher | Self::Schedule => None,
        }
    }

    /// The actor the tracker mutation of a task launch runs under.
    ///
    /// An unattended launch is the orchestrator acting on its own, which the
    /// tracker already has a word for: `system` (`SPEC.md`, "TaskEvent").
    pub fn task_actor(self) -> TaskActor {
        match self {
            Self::User { user_id } => TaskActor::User { user_id },
            Self::Dispatcher | Self::Schedule => TaskActor::System,
        }
    }
}

/// What a launch asks for: the documented create body, already parsed.
///
/// The optional halves are optional in exactly the way `SPEC.md`, "Sessions"
/// documents them: no `base_ref` means the project's integration head or the
/// task's pinned hand-off, no `title` means one derived from the task or the
/// message, and no `message` means nothing is said to the session beyond the
/// generated task message.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    /// The profile to launch, which must be one of this project's.
    pub profile_id: Uuid,
    /// The task to claim, in the form the tracker resolves it.
    pub task: Option<TaskRef>,
    /// The base the session starts from, overriding both the project's
    /// integration head and a task's pinned hand-off.
    pub base_ref: Option<String>,
    /// The title, or `None` to derive one.
    pub title: Option<String>,
    /// The first thing said to this session, after the generated task message.
    pub message: Option<String>,
    /// Whether the task must be in a state this profile serves.
    ///
    /// `false` for a user launch, which is the documented rule of
    /// `ARCHITECTURE.md`, "Task tracker" → "Launching a session for a task":
    /// the user chose the pairing, so every non-terminal state qualifies.
    /// `true` for an unattended launch, which chose the task from a queue and
    /// must not carry a profile out of it.
    pub require_served_state: bool,
}

impl LaunchRequest {
    /// A launch of `profile_id` with everything else left out.
    pub fn new(profile_id: Uuid) -> Self {
        Self {
            profile_id,
            task: None,
            base_ref: None,
            title: None,
            message: None,
            require_served_state: false,
        }
    }

    /// The message, or `None` when there is nothing to say.
    ///
    /// A message of whitespace is no message: it would derive no title, and
    /// [`validate_launch_prompt`] does not count it as an ephemeral session's
    /// prompt either, so it is not sent as input either.
    fn message(&self) -> Option<&str> {
        self.message
            .as_deref()
            .filter(|message| !message.trim().is_empty())
    }
}

/// Create a session and start its launch, and answer the `creating` row.
///
/// The one entry point every launch goes through — the REST route now, the
/// dispatcher and the scheduler when they exist. The order of its decisions,
/// its refusals and its lock discipline are this module's documentation.
///
/// The launch is spawned after the transaction committed, for the reason every
/// other job in this crate is: the launch task re-reads the row it is about to
/// work on (`ARCHITECTURE.md`, "Launch sequence").
pub async fn create_session(
    state: &AppState,
    project_id: Uuid,
    request: LaunchRequest,
    actor: LaunchActor,
) -> Result<Session> {
    let projects = ProjectRepository::new(&state.pool);
    let project = projects.find(project_id).await?.ok_or(Error::NotFound)?;

    // A project with no repository has no base to clone, and one with no
    // `default_branch` has no base to default to. The second cannot happen on a
    // `ready` project — the clone job will not promote one — and is guarded
    // rather than unwrapped.
    if project.status != ProjectStatus::Ready {
        return Err(Error::Conflict(NOT_READY.to_string()));
    }
    let Some(default_branch) = project.default_branch.as_deref() else {
        error!(project_id = %project_id, "a ready project has no default branch");
        return Err(Error::Conflict(NOT_READY.to_string()));
    };

    let profile = projects
        .find_profile(project_id, request.profile_id)
        .await?
        .ok_or_else(|| Error::BadRequest(UNKNOWN_PROFILE.to_string()))?;

    let message = request.message();

    // Read on the pool, for the 404 and for the title: the row the claim is
    // decided against is the one the mutation locks a few lines down.
    let task = match request.task {
        Some(reference) => Some(
            TaskRepository::new(&state.pool)
                .find_task(project_id, reference)
                .await?
                .ok_or(Error::NotFound)?,
        ),
        None => None,
    };

    // A task is a prompt: its generated message is what an ephemeral session
    // runs, so one launched for a task needs no `message` (`SPEC.md`).
    validate_launch_prompt(profile.kind, task.as_ref().map(|task| task.id), message)?;

    let title = match request.title.as_deref() {
        Some(raw) => Some(validate_title(raw)?),
        None => default_title(task.as_ref().map(|task| task.title.as_str()), message),
    };

    // A base the caller named is resolved now, under the project git lock and
    // before any database transaction, so that the documented lock order holds
    // and a name that is not in the mirror is a 400 rather than a session that
    // fails a moment later (ADR 0021). The launcher resolves it again when it
    // clones. A hand-off's base is skipped here: it is a commit this project
    // pinned, chosen under a lock this launch has not taken yet.
    let explicit_base = request.base_ref.as_deref();
    let starts_from_handoff = explicit_base.is_none()
        && task
            .as_ref()
            .is_some_and(|task| task.current_handoff_id.is_some());
    if !starts_from_handoff {
        let base_ref = explicit_base.unwrap_or(default_branch);
        let guard = state.git_locks.lock(project_id).await;
        resolve_base(
            &guard,
            &DataPaths::from_config(&state.config),
            Some(base_ref),
            default_branch,
        )
        .await
        .map_err(|err| {
            debug!(
                project_id = %project_id,
                git.base = %base_ref,
                error = %err,
                "a requested session base ref does not resolve",
            );
            Error::BadRequest(UNRESOLVED_BASE.to_string())
        })?;
    }

    // Before the insert, so the hash the row stores and the only copy of the
    // value are created together (ADR 0029). The row records the name the
    // caller gave, not the commit it resolved to.
    let token = McpToken::generate();

    let (session, task_message) = match task {
        Some(task) => {
            insert_claiming(
                state,
                &profile,
                actor,
                task.id,
                LaunchBase {
                    explicit: explicit_base,
                    default_branch,
                },
                title.as_deref(),
                &token,
                request.require_served_state,
            )
            .await?
        }
        None => {
            let mut new_session = NewSession::new(
                project_id,
                profile.id,
                profile.kind,
                explicit_base.unwrap_or(default_branch),
                token.hash(),
            );
            new_session.created_by = actor.user_id();
            let new_session = new_session.with_title(title.as_deref())?;

            let mut tx = state.pool.begin().await?;
            let session = SessionRepository::new(&state.pool)
                .insert(&mut tx, &new_session)
                .await?;
            tx.commit().await?;

            (session, None)
        }
    };

    info!(
        session_id = %session.id,
        project_id = %project_id,
        profile_id = %profile.id,
        kind = ?profile.kind,
        task_id = ?session.task_id,
        "session created",
    );

    let prompt = first_message(state, &session, actor, task_message.as_deref(), message);
    state
        .launcher()
        .launch(session.id, LaunchMode::Fresh { token, prompt });

    Ok(session)
}

/// What a launch starts from before the task's hand-off has been read.
///
/// The caller's `base_ref` and the project's integration head: which of them
/// the session ends up with — and whether a hand-off commit displaces both —
/// is decided under the lock in [`insert_claiming`].
#[derive(Debug, Clone, Copy)]
struct LaunchBase<'a> {
    /// The `base_ref` the caller named, already resolved in the mirror.
    explicit: Option<&'a str>,
    /// The project's `default_branch`.
    default_branch: &'a str,
}

/// Insert the session row and claim its task in one tracker-locked
/// transaction, and answer with the row and the generated task message.
///
/// The whole of `ARCHITECTURE.md`, "Task tracker" → "Launching a session for a
/// task" that is not the launch itself, in the documented lock order: the
/// project row (the mutation), then the task row, then the session insert
/// (ADR 0021). Nothing here resolves a ref or calls the engine — a git lock is
/// never taken from inside a database one.
///
/// The order within the transaction is forced by the schema:
/// `tasks.lease_holder_session_id` references `sessions`, so the row exists
/// before the claim points at it. Both are the same transaction, so a lost
/// claim takes the session row with it: [`claim_for_launch`] answers 409 `task
/// is not claimable` for a held, blocked or terminal task — the statement's
/// `WHERE` decides all three at once — and `?` drops the mutation, which rolls
/// back. `require_served_state` is refused before the insert, with the same
/// sentence the MCP `claim` tool answers with.
///
/// The base and the hand-off are selected from the *locked* row, so a session
/// cannot start from a hand-off that was superseded between the read and the
/// claim. An explicit `base_ref` overrides the hand-off commit and leaves
/// `handoff_id` null, and the generated message says so.
#[allow(clippy::too_many_arguments)]
async fn insert_claiming(
    state: &AppState,
    profile: &AgentProfile,
    actor: LaunchActor,
    task_id: Uuid,
    base: LaunchBase<'_>,
    title: Option<&str>,
    token: &McpToken,
    require_served_state: bool,
) -> Result<(Session, Option<String>)> {
    // The whole transaction is retried if Postgres aborts it as a deadlock
    // victim: it rolled back whole, so nothing of the session or the claim
    // survived it to be half-applied (`ARCHITECTURE.md`, "Task tracker" →
    // "Lock order").
    retry_on_serialization_failure("launch_session", || async {
        insert_claiming_once(
            state,
            profile,
            actor,
            task_id,
            base,
            title,
            token,
            require_served_state,
        )
        .await
    })
    .await
}

/// One attempt of [`insert_claiming`], from the project lock to the commit.
#[allow(clippy::too_many_arguments)]
async fn insert_claiming_once(
    state: &AppState,
    profile: &AgentProfile,
    actor: LaunchActor,
    task_id: Uuid,
    base: LaunchBase<'_>,
    title: Option<&str>,
    token: &McpToken,
    require_served_state: bool,
) -> Result<(Session, Option<String>)> {
    let project_id = profile.project_id;
    let tasks = TaskRepository::new(&state.pool);

    let mut mutation = TrackerMutation::begin(&state.pool, project_id, actor.task_actor()).await?;

    let task = tasks
        .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(task_id))
        .await?
        .ok_or(Error::NotFound)?;

    // Read under the same lock the claim is decided under, as the MCP `claim`
    // tool reads it: `profile_states` is edited under the project lock, so a
    // list read from the pool could already be stale here.
    if require_served_state {
        let served = tasks
            .list_profile_states_in(mutation.conn(), project_id, profile.id)
            .await?;
        if !served.iter().any(|state| state.id == task.state_id) {
            return Err(Error::Conflict(NOT_SERVED.to_string()));
        }
    }

    let handoff = match task.current_handoff_id {
        Some(id) => {
            tasks
                .find_handoff_in(mutation.conn(), project_id, id)
                .await?
        }
        None => None,
    };
    let comment = match handoff.as_ref().and_then(|handoff| handoff.comment_id) {
        Some(id) => {
            tasks
                .find_comment_in(mutation.conn(), project_id, id)
                .await?
        }
        None => None,
    };

    // The pinned commit is the default base for a task that has one; an
    // explicit base wins and records no hand-off, because the session is not
    // starting from that revision (`SPEC.md`, "Sessions").
    let (base_ref, handoff_id, base_override) = match (base.explicit, handoff.as_ref()) {
        (Some(explicit), handoff) => (
            explicit.to_string(),
            None,
            handoff.is_some().then_some(explicit),
        ),
        (None, Some(handoff)) => (handoff.commit.clone(), Some(handoff.id), None),
        (None, None) => (base.default_branch.to_string(), None, None),
    };

    let task_message = generated_task_message(
        &task,
        handoff.as_ref().map(|handoff| HandoffContext {
            handoff,
            comment: comment.as_ref().map(|comment| comment.body.as_str()),
        }),
        base_override,
    );

    let mut new_session =
        NewSession::new(project_id, profile.id, profile.kind, base_ref, token.hash());
    new_session.created_by = actor.user_id();
    new_session.task_id = Some(task.id);
    new_session.handoff_id = handoff_id;
    let new_session = new_session.with_title(title)?;

    let session = SessionRepository::new(&state.pool)
        .insert(&mut mutation.conn(), &new_session)
        .await?;

    // The claim, its `claimed` event, its `task_sessions` link and the
    // notification the commit issues are all the tracker's.
    claim_for_launch(&mut mutation, &task, session.id).await?;
    mutation.commit().await?;

    Ok((session, Some(task_message)))
}

/// Deliver what this session is told first, and answer with the ephemeral
/// launch prompt.
///
/// Up to two texts, in the one documented order: the generated task message,
/// when the session was launched for a task, and then the caller's `message`
/// (`SPEC.md`, "Sessions").
///
/// The two kinds differ in *where* they go, not in whether they are delivered:
/// an ephemeral session gets them joined by a blank line as the `-p` prompt the
/// returned value carries, a conversational one gets them as inputs queued for
/// the owner that is about to attach. See the module documentation for why the
/// registry entry is created here.
///
/// The generated message is queued with no `user_id`: nobody typed it, so the
/// `user_message` the owner records for it carries `user_id: null`. So does a
/// caller message of an unattended launch, for the same reason.
///
/// A refused submission is logged and not returned: the session exists and is
/// launching, and the caller can send the message again over the socket. No
/// text is ever logged (rule 3).
fn first_message(
    state: &AppState,
    session: &Session,
    actor: LaunchActor,
    task_message: Option<&str>,
    message: Option<&str>,
) -> Option<String> {
    if session.kind == SessionKind::Ephemeral {
        return match (task_message, message) {
            (Some(task_message), Some(message)) => Some(format!("{task_message}\n\n{message}")),
            (Some(text), None) | (None, Some(text)) => Some(text.to_string()),
            (None, None) => None,
        };
    }

    if task_message.is_none() && message.is_none() {
        return None;
    }

    // The receiver is dropped: the launch task's own `register` installs the
    // channel the owner reads, and keeps the queue these submissions fill.
    let _owner = state
        .session_registry
        .register(session.id, session.kind, Phase::Creating);

    if let Some(task_message) = task_message {
        queue_first(state, session, None, task_message);
    }
    if let Some(message) = message {
        queue_first(state, session, actor.user_id(), message);
    }

    None
}

/// Queue one of a new session's first inputs, or log why it was refused.
fn queue_first(state: &AppState, session: &Session, user_id: Option<Uuid>, text: &str) {
    let registry: &SessionRegistry = &state.session_registry;

    match registry.submit(
        session.id,
        QueuedInput {
            input: SessionInput::Message {
                text: text.to_string(),
            },
            user_id,
            client_id: None,
            accepted_at: Utc::now(),
        },
    ) {
        SubmitResult::Rejected(reason) => warn!(
            session_id = %session.id,
            reason = %reason,
            "a first message of a new session was not queued",
        ),
        result => debug!(
            session_id = %session.id,
            result = ?result,
            "a first message of a new session was queued",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three actors differ in exactly two answers, and in nothing else.
    #[test]
    fn only_a_user_launch_names_a_user() {
        let user_id = Uuid::from_u128(11);

        let user = LaunchActor::User { user_id };
        assert_eq!(user.user_id(), Some(user_id));
        assert_eq!(user.task_actor(), TaskActor::User { user_id });

        for unattended in [LaunchActor::Dispatcher, LaunchActor::Schedule] {
            assert_eq!(unattended.user_id(), None, "{unattended:?}");
            assert_eq!(unattended.task_actor(), TaskActor::System, "{unattended:?}");
        }
    }

    /// A message of whitespace is no message at all.
    #[test]
    fn a_blank_message_is_no_message() {
        let mut request = LaunchRequest::new(Uuid::from_u128(3));
        assert_eq!(request.message(), None);

        request.message = Some("   \n ".to_string());
        assert_eq!(request.message(), None);

        request.message = Some("do the thing".to_string());
        assert_eq!(request.message(), Some("do the thing"));
    }
}
