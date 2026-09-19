//! [`GitService`], the composite operations the REST routes, the MCP tools and
//! the session endpoints call.
//!
//! Everything below this module is a primitive: one git command, or one
//! temporary clone, against refs somebody else resolved under a lock somebody
//! else took. This is where those become the operations the product has —
//! sync, diff, session-branch listing, merge, rebase and push — and it is the
//! single code path for humans and agents (ADR 0007). A route hands its
//! request through unchanged and answers whatever comes back; an MCP tool does
//! the same with [`GitActor::Session`] instead of [`GitActor::User`].
//!
//! Each mutating operation does the same six things, in this order:
//!
//! 1. **Validate the refs.** Every name is parsed with [`GitRef::parse`] and
//!    checked against the kind rule for its role, and a session id is checked
//!    against this project's sessions, before anything is locked, fetched or
//!    written. A bad request therefore has no side effects at all
//!    (`SPEC.md`, "Git": 400).
//! 2. **Read the rows.** The project has to be `ready` and a named session has
//!    to be one of its own; both are plain reads with no lock.
//! 3. **Take the project git lock, once,** and hold it through completion
//!    (`ARCHITECTURE.md`, "Git model", Serialization). The primitives take the
//!    guard and never reacquire it.
//! 4. **Sync every participating session** silently, then resolve every ref to
//!    a fixed commit, so the operation works with commits rather than names
//!    that could move under it ("Fetch-back").
//! 5. **Run the primitive** with the credential and the commit identity the
//!    [`GitCredentialProvider`] answers for this actor, which is also what
//!    writes the `secret_uses` audit row (ADR 0002).
//! 6. **Release the lock and record the outcome** as a `git` event on the
//!    sessions that took part (`SPEC.md`, "AgentEvent").
//!
//! **Which sessions get the event.** Every session whose ref took part as
//! `source`, `branch` or `ref`, plus the calling session when the actor is one
//! and is not already among them. An operation naming no session — merging
//! `origin/main` into `main`, say — writes no event and is described entirely
//! by what it returns. Each write is its own short transaction through
//! `SessionRepository::append_events`, which locks the session row, derives
//! `MAX(seq)+1` and issues the `session_events` notification inside it
//! (ADR 0028; `docs/data-model.md`, `events`).
//!
//! **Which failures get one.** The outcome of the primitive, and only that. A
//! ref that does not parse, a session that is not this project's, a session
//! with no work tree yet and a project that is not ready are all refusals of
//! the request itself: they change nothing, so there is nothing to record. A
//! conflict, a non-fast-forward rejection or a failed command *is* an outcome
//! and is recorded with `ok: false`, with `conflicts` or the generic
//! user-facing message in `error` — never git's stderr, never a credential
//! (`CLAUDE.md` rule 3).
//!
//! **What an event write cannot undo.** It happens after the git operation
//! succeeded, so a failure to write it is logged at `error!` and the
//! successful body is still returned: the mirror has already moved, and the
//! panel catches up on its next load.
//!
//! # Ordering
//!
//! The git lock comes first, always (ADR 0021). A caller must not already hold
//! a database transaction that has locked this project's row when it calls any
//! method here, because the lock this waits for may be held by somebody about
//! to want that row. The one method that takes a guard instead of acquiring
//! one is [`GitService::merge_commit`], for the task merge, whose caller holds
//! the lock across the hand-off verification and the merge together
//! (`ARCHITECTURE.md`, "Git model", Merge, rebase, push).

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use uuid::Uuid;

use super::refs::{GitRef, ResolvedRef};
use super::{
    ComparePage, DataPaths, FetchOutcome, GitActor, GitCredentialProvider, GitError,
    MAX_PATCH_BYTES, MergeOutcome, ProjectGitGuard, ProjectGitLocks, PushOutcome, RebaseOutcome,
    diff, integrate, mirror, push, refs, session,
};
use crate::models::{
    Diff, GitMergeDetail, GitPushDetail, GitRebaseDetail, GitSyncDetail, NewEvent, Project,
    ProjectStatus, SessionBranch, SyncOutcome,
};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use crate::tracker::handoffs::TaskHandoffVerifier;

/// The `kind` column every outcome event is written under (`SPEC.md`,
/// "AgentEvent").
const GIT_EVENT_KIND: &str = "git";

/// The four `op` values a `git` event carries.
const OP_SYNC: &str = "sync";
const OP_MERGE: &str = "merge";
const OP_REBASE: &str = "rebase";
const OP_PUSH: &str = "push";

/// The message a project that cannot be operated on answers with (409).
///
/// The same words [`mirror::fetch_project`] uses, and for the same reason: a
/// project that is still cloning has no repository to work in, and one in
/// `error` has nothing anybody should write to.
///
/// `pub(crate)` because hand-off publication refuses an unready project with
/// the identical message before it takes the git lock
/// ([`crate::tracker::handoffs::HandoffService`]), and one spelling of it is
/// one fewer string for a client to have to match twice.
pub(crate) const NOT_READY: &str = "project is not ready";

/// How much life a credential must have left to be worth starting a push with
/// (ADR 0002). Five minutes, as the fetch asks for.
const CREDENTIAL_TTL: Duration = Duration::from_secs(300);

/// What a git primitive answers with; widened into [`Error`] only when the
/// outcome has been recorded.
type GitResult<T> = std::result::Result<T, GitError>;

/// Which side of a diff the caller selected (`SPEC.md`, "Git": exactly one of
/// `head` or `handoff_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSelector {
    /// A ref name: a session id, an integration head or an upstream-tracking
    /// branch. A session is synced before the diff is taken.
    Head(String),
    /// A hand-off id, which selects that hand-off's retained commit in this
    /// project's repository and never syncs a moving branch (ADR 0018).
    Handoff(Uuid),
}

/// The hand-off a task merge was allowed to take, as the verifier answers it.
///
/// A pinned commit rather than a branch: the task form merges exactly the
/// commit the hand-off retained and never a newer session tip (`SPEC.md`,
/// "Git"; `ARCHITECTURE.md`, "Git model", Merge, rebase, push).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedHandoff {
    /// The full object id retained under `refs/handoffs/<id>` (ADR 0018).
    pub commit: String,
    /// The branch the hand-off was published from, for the merge message.
    pub source_branch: String,
    /// The session that published it, when it still exists: the one session
    /// besides the caller that is told about the merge (`SPEC.md`,
    /// "AgentEvent" — the event is written on every session whose work took
    /// part). `None` once that session has been deleted.
    pub source_session_id: Option<Uuid>,
}

/// The source half of [`GitService::merge_commit`]: a commit that is merged
/// without resolving or syncing any branch.
///
/// Three values rather than three parameters, because they are one thing — a
/// pinned source — and a merge already takes a guard, a target, a message and
/// an actor.
#[derive(Debug, Clone, Copy)]
pub struct PinnedSource<'a> {
    /// The full object id to merge.
    pub commit: &'a str,
    /// What the outcome event calls this source (`SPEC.md`, "AgentEvent"):
    /// the hand-off id for a task merge.
    pub label: &'a str,
    /// The sessions whose work took part, which are told about the outcome
    /// beside the calling one.
    pub participants: &'a [Uuid],
}

/// Who decides whether a `{task_id, handoff_id}` pair may be merged.
///
/// The seam between this epic and "Code hand-offs and review": the task merge
/// has to verify *under the project git lock* that the hand-off is the task's
/// current one and that its review status is `approved`
/// (`ARCHITECTURE.md`, "Git model", Merge, rebase, push), and that reads
/// `task_handoffs`, which is the other epic's table. So the check is a hook
/// rather than a query here, and [`GitService::merge_handoff`] calls it with
/// the lock already held, between taking it and merging.
///
/// The contract for an implementation: [`Error::Conflict`] when the hand-off
/// is not the task's current one or is not `approved` (409, `SPEC.md`, "Git"),
/// and [`Error::NotFound`] for a task or hand-off this project does not have.
///
/// `#[async_trait]` for the reason [`GitCredentialProvider`] gives: it is held
/// as an `Arc<dyn …>` and has to stay dyn compatible.
#[async_trait]
pub trait HandoffVerifier: Send + Sync {
    /// The commit `handoff_id` pinned, if it is `task_id`'s current, approved
    /// hand-off in this project.
    async fn approved_commit(
        &self,
        project_id: Uuid,
        task_id: Uuid,
        handoff_id: Uuid,
    ) -> Result<ApprovedHandoff>;
}

/// What a task merge answers while no verifier is installed (409).
const NO_HANDOFFS: &str = "task hand-offs are not available";

/// The verifier a bare [`GitService::new`] carries until one is injected with
/// [`GitService::with_handoff_verifier`].
///
/// It refuses every task merge with the documented conflict rather than
/// pretending to approve one: an orchestrator that cannot read a hand-off's
/// review status must never merge on its behalf. Every production service is
/// built by [`GitService::from_state`], which installs
/// [`TaskHandoffVerifier`](crate::tracker::handoffs::TaskHandoffVerifier)
/// instead, so this is what the git suite's own fixtures see.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHandoffs;

#[async_trait]
impl HandoffVerifier for NoHandoffs {
    async fn approved_commit(&self, _: Uuid, _: Uuid, _: Uuid) -> Result<ApprovedHandoff> {
        Err(Error::Conflict(NO_HANDOFFS.to_string()))
    }
}

/// The composite git operations, over the four things they all need.
///
/// Cheap to build — a pool handle, a path layout and two `Arc`s — so a route
/// or a tool builds one per request with [`GitService::from_state`] rather
/// than the state carrying another field.
#[derive(Clone)]
pub struct GitService {
    pool: PgPool,
    paths: DataPaths,
    locks: Arc<ProjectGitLocks>,
    credentials: Arc<dyn GitCredentialProvider>,
    handoffs: Arc<dyn HandoffVerifier>,
}

impl std::fmt::Debug for GitService {
    /// The provider holds key material and the pool holds connection details,
    /// so neither is rendered here.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitService")
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

impl GitService {
    /// Build the service over the four collaborators it uses.
    pub fn new(
        pool: PgPool,
        paths: DataPaths,
        locks: Arc<ProjectGitLocks>,
        credentials: Arc<dyn GitCredentialProvider>,
    ) -> Self {
        Self {
            pool,
            paths,
            locks,
            credentials,
            handoffs: Arc::new(NoHandoffs),
        }
    }

    /// The same service with `handoffs` deciding its task merges.
    ///
    /// How the real verifier is installed without another [`AppState`] field:
    /// [`GitService::from_state`] builds the service and wraps it, and every
    /// other call site keeps the [`NoHandoffs`] default.
    #[must_use]
    pub fn with_handoff_verifier(mut self, handoffs: Arc<dyn HandoffVerifier>) -> Self {
        self.handoffs = handoffs;
        self
    }

    /// The service the handlers use: the same pool, the same lock table and
    /// the same credential provider the rest of the process shares, with the
    /// real hand-off verifier installed.
    ///
    /// The one composition point, rather than an [`AppState`] field: a
    /// [`TaskHandoffVerifier`](crate::tracker::handoffs::TaskHandoffVerifier)
    /// is a pool handle, so building one per request costs nothing, and every
    /// production caller — the REST routes, the MCP tools, the session
    /// endpoints, the launcher — reaches a service through here.
    ///
    /// It is also the only place `git/` names a `tracker::` type. The
    /// dependency does not invert: [`HandoffVerifier`] and the merge are
    /// entirely this module's, and the tracker's implementation of the trait
    /// is chosen *here* rather than reached for from the merge, which is what
    /// keeps the seam one-way (`ARCHITECTURE.md`, "Git model").
    pub fn from_state(state: &AppState) -> Self {
        Self::new(
            state.pool.clone(),
            DataPaths::from_config(&state.config),
            Arc::clone(&state.git_locks),
            Arc::clone(&state.git_credentials),
        )
        .with_handoff_verifier(Arc::new(TaskHandoffVerifier::new(state.pool.clone())))
    }

    /// Fetch this project's upstream (`ARCHITECTURE.md`, "Git model", Project
    /// clone).
    ///
    /// Beside the composite operations so that every git operation is reached
    /// through one handle; the work itself is [`mirror::fetch_project`]'s, and
    /// the free function stays for the callers that hold an [`AppState`].
    pub async fn fetch_project(
        &self,
        project_id: Uuid,
        actor: &GitActor,
        max_age: Option<Duration>,
    ) -> Result<FetchOutcome> {
        mirror::fetch_project_with(
            &self.pool,
            &self.paths,
            &self.locks,
            self.credentials.as_ref(),
            project_id,
            actor,
            max_age,
        )
        .await
    }

    /// Fetch one session's branch into the project repository and record it.
    ///
    /// `POST /sessions/{id}/sync` and `POST /sessions/{id}/end`
    /// (`SPEC.md`, "Sessions"). The `git` event with `op: "sync"` is what
    /// distinguishes this from the silent fetch-back every other operation
    /// does first: an explicit sync is a thing the user asked for and the
    /// "Changes" panel refreshes on it (`ARCHITECTURE.md`, "Git model",
    /// Diff).
    ///
    /// # Errors
    ///
    /// - [`Error::NotFound`] — no such project.
    /// - [`Error::Conflict`] `project is not ready`.
    /// - [`Error::BadRequest`] — the session is not this project's.
    /// - [`Error::Git`] with [`GitError::UnknownRef`] — the session has no
    ///   work tree yet and no ref in the repository either, which is what a
    ///   session still in `creating` looks like. A 400, and no event: nothing
    ///   happened.
    pub async fn sync_session(
        &self,
        project_id: Uuid,
        session_id: Uuid,
        actor: &GitActor,
    ) -> Result<SyncOutcome> {
        self.ready_project(project_id).await?;
        self.require_session(project_id, session_id).await?;

        let outcome = {
            let guard = self.locks.lock(project_id).await;
            self.sync_session_silent(&guard, session_id).await
        };

        let git_ref = refs::session_ref(session_id);
        let targets = self.event_targets(&[session_id], actor);

        match outcome {
            Ok(commit) => {
                self.record(
                    &targets,
                    OP_SYNC,
                    true,
                    &GitSyncDetail {
                        git_ref: git_ref.clone(),
                        commit: Some(commit.clone()),
                        error: None,
                    },
                )
                .await;

                Ok(SyncOutcome { git_ref, commit })
            }
            // Nothing to sync is a refusal of the request, not an outcome:
            // the session has never had a work tree, so no event describes it.
            Err(error @ Error::Git(GitError::UnknownRef(_))) => Err(error),
            Err(error) => {
                self.record(
                    &targets,
                    OP_SYNC,
                    false,
                    &GitSyncDetail {
                        git_ref,
                        commit: None,
                        error: Some(error.user_message()),
                    },
                )
                .await;

                Err(error)
            }
        }
    }

    /// Fetch one session's branch into the project repository without
    /// recording anything, and answer the commit `refs/sessions/<sid>` now
    /// points at.
    ///
    /// What every operation involving a session does first, and what the diff
    /// endpoint and hand-off publication use directly: a silent sync cannot
    /// re-trigger the panel's refresh-on-`git`-event rule
    /// (`ARCHITECTURE.md`, "Git model", Fetch-back and Diff). The caller holds
    /// the lock and passes the guard.
    ///
    /// **A missing work directory is not a failure.** A `done` or `failed`
    /// session whose directory has been deleted still owns
    /// `refs/sessions/<sid>` in the project repository, and merging or pushing
    /// that ref must keep working. When the directory is gone but the ref is
    /// there, the sync is skipped with a `debug!` and the ref's current commit
    /// is answered. Only when neither exists is this
    /// [`GitError::UnknownRef`] — the session has produced nothing yet.
    pub async fn sync_session_silent(
        &self,
        guard: &ProjectGitGuard,
        session_id: Uuid,
    ) -> Result<String> {
        if self.paths.session_work(session_id).exists() {
            return Ok(session::fetch_back(guard, &self.paths, session_id).await?);
        }

        let repo = self.paths.project_repo(guard.project_id());
        match refs::resolve(&repo, &GitRef::Session(session_id)).await {
            Ok(resolved) => {
                debug!(
                    session_id = %session_id,
                    commit = %resolved.commit,
                    "the session work clone is gone; using the ref already in the project repository"
                );
                Ok(resolved.commit)
            }
            Err(GitError::UnknownRef(_) | GitError::NotACommit(_)) => Err(Error::Git(
                GitError::UnknownRef(session::session_branch(session_id)),
            )),
            Err(err) => Err(err.into()),
        }
    }

    /// The project's session branches with their ahead/behind counts
    /// (`SPEC.md`, "Git"; the `list_session_branches` MCP tool).
    ///
    /// Read-only and silent: no sync, no event (`ARCHITECTURE.md`, "MCP
    /// design", Side effects). The lock is taken all the same, briefly, so
    /// that a write-back running at the same moment cannot produce a listing
    /// in which one branch's commit and another's counts come from different
    /// moments.
    pub async fn list_session_branches(&self, project_id: Uuid) -> Result<Vec<SessionBranch>> {
        let (_, default_branch) = self.ready_project(project_id).await?;

        let guard = self.locks.lock(project_id).await;
        let branches = diff::session_branches(&self.paths, project_id, &default_branch).await?;
        drop(guard);

        Ok(branches)
    }

    /// The diff from `merge-base(base, head)` to `head`
    /// (`GET /projects/{pid}/git/diff`).
    ///
    /// A [`DiffSelector::Head`] naming a session is synced silently first, so
    /// the panel shows what the agent has committed; a
    /// [`DiffSelector::Handoff`] is checked against `task_handoffs` for this
    /// project (404 otherwise) and then resolves `refs/handoffs/<id>` in this
    /// project's own repository, never syncing anything
    /// (`ARCHITECTURE.md`, "Git model", Diff). `base` defaults to the
    /// project's default branch. Both refs are resolved under the lock and the
    /// diff itself runs without it, on the fixed commits.
    pub async fn diff(
        &self,
        project_id: Uuid,
        selector: DiffSelector,
        base: Option<&str>,
    ) -> Result<Diff> {
        let (_, default_branch) = self.ready_project(project_id).await?;

        let head_ref = match &selector {
            DiffSelector::Head(name) => {
                let parsed = GitRef::parse(name)?;
                require_kind(is_diff_head(&parsed), &parsed)?;
                parsed
            }
            DiffSelector::Handoff(id) => {
                // A hand-off is the URL project's or it does not exist
                // (`SPEC.md`, "Git"). The scope comes through the task the
                // record belongs to, as a plain read outside any transaction
                // and before the lock: the lock is taken only to resolve.
                TaskRepository::new(&self.pool)
                    .find_handoff(project_id, *id)
                    .await?
                    .ok_or(Error::NotFound)?;
                GitRef::Handoff(*id)
            }
        };
        let base_ref = match base {
            Some(name) => {
                let parsed = GitRef::parse(name)?;
                require_kind(parsed.is_base(), &parsed)?;
                parsed
            }
            None => GitRef::parse(&format!("refs/heads/{default_branch}"))?,
        };

        let head_session = self.participating_session(project_id, &head_ref).await?;
        let repo = self.paths.project_repo(project_id);

        let (base_resolved, head_resolved) = {
            let guard = self.locks.lock(project_id).await;
            if let Some(session_id) = head_session {
                self.sync_session_silent(&guard, session_id).await?;
            }

            (
                refs::resolve(&repo, &base_ref).await?,
                refs::resolve(&repo, &head_ref).await?,
            )
        };

        Ok(diff::diff(
            &self.paths,
            project_id,
            &base_resolved,
            &head_resolved,
            MAX_PATCH_BYTES,
        )
        .await?)
    }

    /// Merge one branch into an integration head
    /// (`POST /projects/{pid}/git/merge`, branch form; the `merge` MCP tool).
    ///
    /// `source` is anything a merge may take — an integration head, an
    /// upstream-tracking ref, a session ref, a hand-off ref or a commit id —
    /// and `target` is an integration head. A session source is synced first,
    /// so the merge takes the agent's latest commit.
    pub async fn merge_branch(
        &self,
        project_id: Uuid,
        source: &str,
        target: &str,
        message: Option<&str>,
        actor: &GitActor,
    ) -> Result<MergeOutcome> {
        let source_ref = GitRef::parse(source)?;
        require_kind(source_ref.is_merge_source(), &source_ref)?;
        let target_ref = GitRef::parse(target)?;
        require_kind(matches!(target_ref, GitRef::Head(_)), &target_ref)?;

        self.ready_project(project_id).await?;
        let source_session = self.participating_session(project_id, &source_ref).await?;
        let repo = self.paths.project_repo(project_id);

        let outcome = {
            let guard = self.locks.lock(project_id).await;
            if let Some(session_id) = source_session {
                self.sync_session_silent(&guard, session_id).await?;
            }

            let resolved = refs::resolve(&repo, &source_ref).await?;
            self.run_merge(&guard, &resolved, &target_ref, message, actor)
                .await?
        };

        let targets = self.event_targets(source_session.as_slice(), actor);
        self.record_merge(
            outcome,
            &source_ref.api_name(),
            &target_ref.api_name(),
            actor,
            &targets,
        )
        .await
    }

    /// Merge one pinned commit into an integration head, under a lock the
    /// caller already holds.
    ///
    /// The task-merge building block (`ARCHITECTURE.md`, "Git model", Merge,
    /// rebase, push): the hand-off epic verifies under this very guard that
    /// the commit is the task's current, approved hand-off and then calls
    /// this, which is why nothing here syncs — the source is a commit, not a
    /// branch that may have moved. [`PinnedSource`] is the commit, the name
    /// the outcome event gives it and the sessions it belongs to.
    pub async fn merge_commit(
        &self,
        guard: &ProjectGitGuard,
        source: PinnedSource<'_>,
        target: &str,
        message: Option<&str>,
        actor: &GitActor,
    ) -> Result<MergeOutcome> {
        let target_ref = GitRef::parse(target)?;
        require_kind(matches!(target_ref, GitRef::Head(_)), &target_ref)?;

        // Through the parser rather than trusted: it is the one place that
        // knows what an object id looks like, and this one reaches argv.
        let source_ref = GitRef::parse(source.commit)?;
        let GitRef::Commit(commit) = source_ref else {
            return Err(GitError::InvalidRef(source.commit.to_string()).into());
        };

        let resolved = ResolvedRef {
            git_ref: GitRef::Commit(commit.clone()),
            commit,
        };
        let outcome = self
            .run_merge(guard, &resolved, &target_ref, message, actor)
            .await?;

        let targets = self.event_targets(source.participants, actor);
        self.record_merge(
            outcome,
            source.label,
            &target_ref.api_name(),
            actor,
            &targets,
        )
        .await
    }

    /// Merge a task's approved hand-off into an integration head
    /// (`POST /projects/{pid}/git/merge`, task form; the `merge` MCP tool).
    ///
    /// The whole point is the order: the project git lock is taken *first*,
    /// the hand-off is verified under it, and the merge runs before it is
    /// released, so the current-hand-off check and the target write are
    /// serialized against hand-off changes (`ARCHITECTURE.md`, "Git model",
    /// Merge, rebase, push). Nothing is synced: the source is the commit the
    /// hand-off pinned, not a branch that may have moved since.
    ///
    /// A stale or unapproved hand-off is 409 and leaves the target untouched;
    /// the refusal comes from the [`HandoffVerifier`], which
    /// [`GitService::from_state`] makes
    /// [`TaskHandoffVerifier`](crate::tracker::handoffs::TaskHandoffVerifier).
    ///
    /// Without an explicit `message` the merge commit reads `Merge handoff
    /// <id> (<source_branch>) into <target>`: the hand-off id is what the API
    /// names this source by, and the branch beside it is what makes the
    /// history readable (`SPEC.md`, "Git").
    pub async fn merge_handoff(
        &self,
        project_id: Uuid,
        task_id: Uuid,
        handoff_id: Uuid,
        target: &str,
        message: Option<&str>,
        actor: &GitActor,
    ) -> Result<MergeOutcome> {
        // Before the lock and before the verification, so a target of the
        // wrong kind refuses the request with no side effects at all.
        let target_ref = GitRef::parse(target)?;
        require_kind(matches!(target_ref, GitRef::Head(_)), &target_ref)?;

        self.ready_project(project_id).await?;

        let guard = self.locks.lock(project_id).await;
        let approved = self
            .handoffs
            .approved_commit(project_id, task_id, handoff_id)
            .await?;

        // Built here rather than left to the generic default, which would name
        // the bare commit id: the hand-off and its branch are what a reader of
        // the integration branch's history needs. A blank message is treated
        // as none, exactly as `integrate::merge` treats it.
        let message = message
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "Merge handoff {handoff_id} ({}) into {}",
                    approved.source_branch,
                    target_ref.api_name()
                )
            });

        // The hand-off id, which is what the API calls this source
        // (`SPEC.md`, "AgentEvent": "except a task merge's `source`, which is
        // the hand-off id").
        let label = handoff_id.to_string();

        self.merge_commit(
            &guard,
            PinnedSource {
                commit: &approved.commit,
                label: &label,
                participants: approved.source_session_id.as_slice(),
            },
            target,
            Some(&message),
            actor,
        )
        .await
    }

    /// Rebase a branch onto another (`POST /projects/{pid}/git/rebase`; the
    /// `rebase` MCP tool).
    ///
    /// `branch` is an integration head or a session ref, `onto` an integration
    /// head or an upstream-tracking ref. A session branch is synced first and
    /// its checkout reconciled afterwards, which is what the event's
    /// `detail.work_tree` reports (`SPEC.md`, "AgentEvent").
    pub async fn rebase(
        &self,
        project_id: Uuid,
        branch: &str,
        onto: &str,
        actor: &GitActor,
    ) -> Result<RebaseOutcome> {
        let branch_ref = GitRef::parse(branch)?;
        require_kind(branch_ref.is_mutation_target(), &branch_ref)?;
        let onto_ref = GitRef::parse(onto)?;
        require_kind(onto_ref.is_base(), &onto_ref)?;

        self.ready_project(project_id).await?;
        let branch_session = self.participating_session(project_id, &branch_ref).await?;
        let repo = self.paths.project_repo(project_id);

        let outcome = {
            let guard = self.locks.lock(project_id).await;
            if let Some(session_id) = branch_session {
                self.sync_session_silent(&guard, session_id).await?;
            }

            let branch_resolved = refs::resolve(&repo, &branch_ref).await?;
            let onto_resolved = refs::resolve(&repo, &onto_ref).await?;
            let identity = self.credentials.commit_identity(project_id).await?;

            integrate::rebase(
                &guard,
                &self.paths,
                &branch_resolved,
                &onto_resolved,
                &identity,
            )
            .await
        };

        let targets = self.event_targets(branch_session.as_slice(), actor);
        let mut detail = GitRebaseDetail {
            branch: branch_ref.api_name(),
            onto: onto_ref.api_name(),
            commit: None,
            conflicts: None,
            work_tree: None,
            requested_by: integrate::requested_by(actor),
            error: None,
        };

        match outcome {
            Ok(outcome) => {
                detail.commit = Some(outcome.commit.clone());
                detail.work_tree = Some(outcome.work_tree);
                self.record(&targets, OP_REBASE, true, &detail).await;
                Ok(outcome)
            }
            Err(err) => {
                let error = Error::from(err);
                detail.conflicts = error.conflicts();
                detail.error = Some(error.user_message());
                self.record(&targets, OP_REBASE, false, &detail).await;
                Err(error)
            }
        }
    }

    /// Send one ref upstream (`POST /projects/{pid}/git/push`; the `push` MCP
    /// tool).
    ///
    /// `r#ref` is an integration head or a session ref — the only two
    /// namespaces Mars publishes (ADR 0007) — and a session's is synced first.
    /// `remote_branch` defaults to the source's own name, `session/<sid>` for
    /// a session. The credential comes from the provider under this actor,
    /// which is what writes the `secret_uses` row
    /// (`docs/data-model.md`, `secret_uses`).
    pub async fn push(
        &self,
        project_id: Uuid,
        r#ref: &str,
        remote_branch: Option<&str>,
        force: bool,
        actor: &GitActor,
    ) -> Result<PushOutcome> {
        let source_ref = GitRef::parse(r#ref)?;
        require_kind(source_ref.is_push_source(), &source_ref)?;
        // Before the lock, so a malformed upstream branch name refuses the
        // request without syncing anything.
        let planned_branch = match remote_branch {
            Some(requested) => push::validate_remote_branch(requested)?,
            None => push::default_remote_branch(&source_ref)?,
        };

        let (project, default_branch) = self.ready_project(project_id).await?;
        let source_session = self.participating_session(project_id, &source_ref).await?;
        let repo = self.paths.project_repo(project_id);

        let outcome = {
            let guard = self.locks.lock(project_id).await;
            if let Some(session_id) = source_session {
                self.sync_session_silent(&guard, session_id).await?;
            }

            let resolved = refs::resolve(&repo, &source_ref).await?;
            let credential = self
                .credentials
                .credential_for(project_id, actor, CREDENTIAL_TTL)
                .await?;

            push::push(
                &guard,
                &self.paths,
                &resolved,
                Some(planned_branch.as_str()),
                force,
                credential.as_ref(),
                Some(ComparePage {
                    remote_url: &project.remote_url,
                    default_branch: &default_branch,
                }),
            )
            .await
        };

        let targets = self.event_targets(source_session.as_slice(), actor);
        let mut detail = GitPushDetail {
            git_ref: source_ref.api_name(),
            remote_branch: planned_branch,
            commit: None,
            force,
            compare_url: None,
            requested_by: integrate::requested_by(actor),
            error: None,
        };

        match outcome {
            Ok(outcome) => {
                detail.commit = Some(outcome.commit.clone());
                detail.compare_url = outcome.compare_url.clone();
                self.record(&targets, OP_PUSH, true, &detail).await;
                Ok(outcome)
            }
            Err(err) => {
                let error = Error::from(err);
                detail.error = Some(error.user_message());
                self.record(&targets, OP_PUSH, false, &detail).await;
                Err(error)
            }
        }
    }

    /// Resolve the target, obtain the bot identity and run the merge.
    ///
    /// The outer [`Result`] is everything that happened *before* the merge and
    /// therefore records no event; the inner one is the merge's own outcome,
    /// which always does.
    async fn run_merge(
        &self,
        guard: &ProjectGitGuard,
        source: &ResolvedRef,
        target_ref: &GitRef,
        message: Option<&str>,
        actor: &GitActor,
    ) -> Result<GitResult<MergeOutcome>> {
        let repo = self.paths.project_repo(guard.project_id());
        let target = refs::resolve(&repo, target_ref).await?;
        let identity = self.credentials.commit_identity(guard.project_id()).await?;

        Ok(integrate::merge(
            guard,
            &self.paths,
            source,
            &target,
            message,
            &identity,
            actor,
        )
        .await)
    }

    /// Record a merge's outcome and hand it back.
    async fn record_merge(
        &self,
        outcome: GitResult<MergeOutcome>,
        source: &str,
        target: &str,
        actor: &GitActor,
        targets: &[Uuid],
    ) -> Result<MergeOutcome> {
        let mut detail = GitMergeDetail {
            source: source.to_string(),
            target: target.to_string(),
            commit: None,
            fast_forward: None,
            conflicts: None,
            requested_by: integrate::requested_by(actor),
            error: None,
        };

        match outcome {
            Ok(outcome) => {
                detail.commit = Some(outcome.commit.clone());
                detail.fast_forward = Some(outcome.fast_forward);
                self.record(targets, OP_MERGE, true, &detail).await;
                Ok(outcome)
            }
            Err(err) => {
                let error = Error::from(err);
                detail.conflicts = error.conflicts();
                detail.error = Some(error.user_message());
                self.record(targets, OP_MERGE, false, &detail).await;
                Err(error)
            }
        }
    }

    /// The project row, and its default branch, for an operation that may run.
    ///
    /// Before any lock, like [`mirror::fetch_project`]: a project that is
    /// still cloning holds the git lock for the length of that clone, and
    /// waiting minutes to answer 409 helps nobody. A `ready` project always
    /// has a `default_branch` (`docs/data-model.md`, `projects`); a row that
    /// somehow does not is in no state to be merged into either.
    async fn ready_project(&self, project_id: Uuid) -> Result<(Project, String)> {
        let project = ProjectRepository::new(&self.pool)
            .find(project_id)
            .await?
            .ok_or(Error::NotFound)?;

        if project.status != ProjectStatus::Ready {
            return Err(Error::Conflict(NOT_READY.to_string()));
        }

        let default_branch = project
            .default_branch
            .clone()
            .ok_or_else(|| Error::Conflict(NOT_READY.to_string()))?;

        Ok((project, default_branch))
    }

    /// The session behind a ref, when the ref is a session's, checked against
    /// this project.
    ///
    /// A plain read with no `FOR UPDATE`, before the lock: an id that names no
    /// session of this project is the caller's mistake and answers 400, the
    /// same class of answer an unresolvable ref gets (`SPEC.md`, "Git").
    async fn participating_session(
        &self,
        project_id: Uuid,
        git_ref: &GitRef,
    ) -> Result<Option<Uuid>> {
        let GitRef::Session(session_id) = git_ref else {
            return Ok(None);
        };

        self.require_session(project_id, *session_id).await?;
        Ok(Some(*session_id))
    }

    /// Refuse a session id that is not this project's.
    async fn require_session(&self, project_id: Uuid, session_id: Uuid) -> Result<()> {
        SessionRepository::new(&self.pool)
            .find_in_project(project_id, session_id)
            .await?
            .ok_or_else(|| Error::BadRequest(format!("unknown session {session_id}")))?;

        Ok(())
    }

    /// The sessions an outcome event is written on: the participants, plus the
    /// calling session when it is not already one of them.
    fn event_targets(&self, participants: &[Uuid], actor: &GitActor) -> Vec<Uuid> {
        let calling = match actor {
            GitActor::Session(session_id) => Some(*session_id),
            GitActor::User(_) | GitActor::System => None,
        };

        let mut targets: Vec<Uuid> = Vec::with_capacity(participants.len() + 1);
        for session_id in participants.iter().copied().chain(calling) {
            if !targets.contains(&session_id) {
                targets.push(session_id);
            }
        }

        targets
    }

    /// Write one `git` event per target session.
    ///
    /// One transaction each, through the shared append path, after the git
    /// lock has been released (ADR 0021, ADR 0028). A write that fails is
    /// logged and does not fail the operation: the git side already happened,
    /// and answering an error for a missing transcript line would be a worse
    /// lie than the missing line.
    async fn record<D: Serialize>(&self, targets: &[Uuid], op: &str, ok: bool, detail: &D) {
        if targets.is_empty() {
            return;
        }

        let detail = match serde_json::to_value(detail) {
            Ok(detail) => detail,
            Err(err) => {
                error!(op, error = %err, "a git outcome detail could not be serialised");
                return;
            }
        };
        let payload = serde_json::json!({ "op": op, "ok": ok, "detail": detail });

        for &session_id in targets {
            if let Err(err) = self.append_git_event(session_id, &payload).await {
                error!(
                    session_id = %session_id,
                    op,
                    error = %err,
                    "the git outcome event could not be recorded"
                );
            }
        }
    }

    /// One `git` event, in its own transaction.
    async fn append_git_event(&self, session_id: Uuid, payload: &serde_json::Value) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        SessionRepository::new(&self.pool)
            .append_events(
                &mut tx,
                session_id,
                &[NewEvent::now(GIT_EVENT_KIND, payload.clone())],
            )
            .await?;
        tx.commit().await?;

        Ok(())
    }
}

/// May this ref be a diff `head`? A session's work, an integration head or an
/// upstream-tracking ref (`SPEC.md`, "Git"); a hand-off is selected by
/// `handoff_id` instead and a tag is not a branch.
fn is_diff_head(git_ref: &GitRef) -> bool {
    matches!(
        git_ref,
        GitRef::Head(_) | GitRef::Upstream(_) | GitRef::Session(_)
    )
}

/// Refuse a ref whose kind is wrong for the role it was given, naming it as
/// the caller spelled it (400).
fn require_kind(allowed: bool, git_ref: &GitRef) -> GitResult<()> {
    if allowed {
        Ok(())
    } else {
        Err(GitError::InvalidRef(git_ref.api_name()))
    }
}
