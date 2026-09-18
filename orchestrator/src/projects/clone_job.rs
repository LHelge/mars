//! The background job that turns a freshly created project into a ready one.
//!
//! `POST /projects` and `POST /projects/{id}/retry-clone` both leave the row
//! at `status = cloning` and hand the work to [`spawn`]; nothing in a request
//! waits for a clone, which may take minutes on a large repository
//! (`SPEC.md`, "Projects"). What the job does is `ARCHITECTURE.md`, "Git
//! model" (Project clone), and all of it is one call:
//! [`init_project_repo`](crate::git::init_project_repo) is the bare `init`,
//! the `origin` configuration, the symbolic-`HEAD` discovery, the first fetch,
//! the seeding of one integration head per fetched upstream branch and the
//! `HEAD` that ends up pointing at the default one. This module is the
//! orchestration around it: the lock, the row, the directories, the
//! credential, and the single guarded write that publishes the outcome.
//!
//! **Serialization.** Everything happens under the project's git lock, taken
//! once and held to the end (`ARCHITECTURE.md`, "Git model", Serialization).
//! The row is re-read under it, so two jobs racing on one project — a
//! `retry-clone` arriving while the first is still running — cannot both do
//! the work: the second finds a status that is no longer `cloning`, or finds
//! nothing at all, and stops. Project deletion takes the same lock, which is
//! what makes "deleted mid-clone" a clean case rather than a race
//! (see [`run`]). The database transaction that publishes the outcome is
//! opened while the git lock is held, never the other way round (ADR 0021).
//!
//! **Outcomes.** Exactly one of [`ProjectRepository::mark_ready`] and
//! [`ProjectRepository::mark_error`] runs, both guarded by
//! `status = 'cloning'` in their `WHERE` clause. A failure is a `status_message`
//! the user reads, so it is a single line, bounded in length, and scrubbed of
//! any `userinfo` that a URL in git's own words might carry — the credential
//! never reaches a [`GitError`] to begin with (`ARCHITECTURE.md`, "Git model",
//! Credentials), and the scrub is the second line of defence that makes that
//! independent of the git layer's promise (`CLAUDE.md`, rule 3).
//!
//! **Idempotent.** `retry-clone` simply runs this again: the layout helper
//! completes a half-built directory, the repository of an interrupted attempt
//! is discarded before the new one is initialised, and the initialisation
//! itself never moves an integration head.

use std::time::Duration;

use uuid::Uuid;

use crate::git::{
    DataPaths, GitActor, GitError, ProjectGitGuard, init_project_repo, remove_project_repo,
};
use crate::models::{BranchName, Project, ProjectStatus, RemoteUrl};
use crate::prelude::*;
use crate::repositories::ProjectRepository;

/// How much life a credential must have left to be worth starting a clone
/// with.
///
/// The same five minutes the recurring fetch asks for
/// (`crate::git::mirror`): a PAT has no expiry the orchestrator can see, so
/// this only matters to a future minting provider (ADR 0002).
const CREDENTIAL_TTL: Duration = Duration::from_secs(300);

/// The longest `status_message` this job writes, in characters.
///
/// The column is unconstrained `TEXT` (`docs/data-model.md`, `projects`), but
/// the message is shown in a project list, and git's stderr on a failed clone
/// of a misconfigured remote can run to pages.
const MAX_STATUS_MESSAGE: usize = 1000;

/// What a truncated message ends with, counting towards
/// [`MAX_STATUS_MESSAGE`].
const ELLIPSIS: char = '…';

/// Upstream is an empty repository: no branch, so no integration head and no
/// default.
const EMPTY_REMOTE: &str = "remote has no branches";

/// Upstream has branches but names no default, and the project did not either.
const NO_DEFAULT_BRANCH: &str =
    "could not discover the remote default branch; set default_branch explicitly";

/// The project's directory could not be created. An internal fault — a full or
/// read-only volume — so the detail goes to the log and the user is told what
/// it means for the project (`CLAUDE.md`, "Backend conventions").
const LAYOUT_FAILED: &str = "the project's data directory could not be created";

/// The repository of an interrupted earlier attempt could not be removed.
const REPO_NOT_PREPARED: &str = "the project repository directory could not be prepared";

/// The stored `remote_url` does not parse. Only reachable through a row that
/// was written around the model, so it is a fault rather than user error, but
/// it must not panic the job.
const INVALID_REMOTE: &str = "the stored remote URL is not a usable git remote";

/// The credential lookup itself failed: the secret could not be decrypted, or
/// its audit row could not be written. The value is never in the message.
const CREDENTIAL_FAILED: &str = "the project's git credential could not be read";

/// A git failure that rendered to nothing at all; better than an empty reason.
const UNKNOWN_FAILURE: &str = "the git command failed without saying why";

/// What a URL's `userinfo` is replaced with, including the `@` it ended at.
const REDACTED_USERINFO: &str = "***@";

/// The separator a URL's authority — and therefore any `userinfo` — follows.
const SCHEME_SEPARATOR: &str = "://";

/// How often [`wait_for_clone`] re-reads the row.
#[cfg(feature = "integration-tests")]
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Run the clone of `project_id` on its own task.
///
/// What the two routes that create or retry a project call, after their own
/// transaction has committed: the row must be `cloning` and visible before the
/// job re-reads it. The handle is returned for a caller that wants to await the
/// job — the tests do — and dropping it is the ordinary case, because the job
/// reports its outcome by writing the row and not by returning.
///
/// `requested_by` is the user whose request caused this, or `None` when
/// nothing did: it becomes the `secret_uses` actor of the credential lookup
/// (`docs/data-model.md`, `secret_uses`).
pub fn spawn(
    state: AppState,
    project_id: Uuid,
    requested_by: Option<Uuid>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { run(&state, project_id, requested_by).await })
}

/// Clone the project, and write down how it went.
///
/// Infallible on purpose: there is nobody to return an error to — the request
/// that started this has long been answered — so every outcome ends up in the
/// project row and in the log. It also never panics, which is what keeps a
/// [`spawn`]ed job from turning a single bad project into a `JoinError`
/// nothing is watching for.
///
/// The steps are `ARCHITECTURE.md`, "Git model" (Project clone) in order:
/// take the git lock, re-read the row, create the directories, discard any
/// repository an interrupted attempt left, obtain the credential, initialise
/// the repository, and publish `ready` or `error`.
///
/// Four things make it stop without writing anything, all of them normal:
/// the project is gone, the project is no longer `cloning` (somebody retried
/// or finished it), the row could not be read at all, and the final guarded
/// update matching no row — the last one is a project deleted while this ran,
/// and it takes the directory the job created with it.
#[instrument(
    name = "project_clone",
    level = "info",
    skip_all,
    fields(project_id = %project_id)
)]
pub async fn run(state: &AppState, project_id: Uuid, requested_by: Option<Uuid>) {
    // First and held to the end; every database transaction below is opened
    // under it and never the reverse (ADR 0021).
    let guard = state.git_locks.lock(project_id).await;

    let project = match ProjectRepository::new(&state.pool).find(project_id).await {
        Ok(Some(project)) => project,
        Ok(None) => {
            debug!("the project was deleted before its clone could start");
            return;
        }
        Err(error) => {
            error!(%error, "the project row could not be read; the clone is abandoned");
            return;
        }
    };

    if project.status != ProjectStatus::Cloning {
        debug!(status = ?project.status, "the project is not cloning; another job owns it");
        return;
    }

    let outcome = clone_repository(state, &guard, &project, requested_by).await;

    publish(state, project_id, outcome).await;
}

/// Everything between the lock and the row write: the directories, the
/// credential and the initialisation.
///
/// The error is the finished `status_message` rather than a failure type,
/// because there is exactly one thing the caller does with it and every arm
/// has already logged the detail it is not allowed to show.
async fn clone_repository(
    state: &AppState,
    guard: &ProjectGitGuard,
    project: &Project,
    requested_by: Option<Uuid>,
) -> std::result::Result<BranchName, String> {
    let paths = DataPaths::from_config(&state.config);
    let layout = state.config.project_layout(project.id);

    // Idempotent, so a layout a previous attempt half-built is completed
    // rather than refused. `repo.git` is not its to create.
    layout.ensure_created().await.map_err(|error| {
        error!(%error, "the project's data directory could not be created");
        LAYOUT_FAILED.to_string()
    })?;

    // A repository left by an interrupted attempt is discarded rather than
    // reasoned about: initialisation is idempotent, but a `repo.git` a killed
    // process was halfway through configuring is cheaper to throw away than to
    // reconcile. Nothing of Mars's is lost, because a project that never
    // became `ready` has no integration work in it — `mark_error` and
    // `mark_ready` are both guarded by `status = 'cloning'`, so a project that
    // reached `ready` never comes back here.
    remove_project_repo(guard, &paths).await.map_err(|error| {
        error!(%error, "a half-written project repository could not be removed");
        REPO_NOT_PREPARED.to_string()
    })?;

    let remote_url = RemoteUrl::parse(&project.remote_url).map_err(|error| {
        error!(%error, "the stored remote url is not a usable git remote");
        INVALID_REMOTE.to_string()
    })?;

    // Under the lock, like the fetch's (`crate::git::mirror::fetch_project`):
    // the provider writes the `secret_uses` row with `purpose = 'git'` itself,
    // naming the user who asked or nothing at all for a job nobody did.
    let credential = state
        .git_credentials
        .credential_for(project.id, &actor_for(requested_by), CREDENTIAL_TTL)
        .await
        .map_err(|error| {
            error!(%error, "the project's git credential could not be read");
            CREDENTIAL_FAILED.to_string()
        })?;

    let outcome = init_project_repo(
        guard,
        &paths,
        &remote_url,
        project.default_branch.as_deref(),
        credential.as_ref(),
    )
    .await
    .map_err(|error| {
        // `warn`, not `error`: an unreachable remote or a branch that is not
        // there is the user's to fix, and the row they are about to read says
        // the same thing.
        warn!(%error, "the project repository could not be initialised");
        describe(&error)
    })?;

    // A discovered branch has only been through `ls-remote`'s parser, so it
    // still has to be a name the column accepts; a requested one already was.
    BranchName::parse(&outcome.default_branch).map_err(|error| {
        error!(%error, "the default branch is not a usable branch name");
        sanitize(&format!(
            "the default branch {:?} is not a usable branch name",
            outcome.default_branch
        ))
    })
}

/// Write the outcome to the project row, once, under the lock the caller still
/// holds.
///
/// Both statements are guarded by `status = 'cloning'`, so a project somebody
/// else moved on or deleted is not overwritten. Nothing matching is not a
/// failure: it means the row is gone, and then the directories this job
/// created are the job's to remove.
async fn publish(
    state: &AppState,
    project_id: Uuid,
    outcome: std::result::Result<BranchName, String>,
) {
    let projects = ProjectRepository::new(&state.pool);

    let written: Result<u64> = async {
        let mut tx = state.pool.begin().await?;
        let rows = match &outcome {
            Ok(branch) => projects.mark_ready(&mut tx, project_id, branch).await?,
            Err(message) => projects.mark_error(&mut tx, project_id, message).await?,
        };
        tx.commit().await?;
        Ok(rows)
    }
    .await;

    match written {
        Err(error) => {
            error!(%error, "the clone outcome could not be recorded");
        }
        Ok(0) => discard(state, project_id).await,
        Ok(_) => match &outcome {
            Ok(branch) => info!(git.default_branch = %branch, "the project is ready"),
            Err(reason) => warn!(%reason, "the project clone failed"),
        },
    }
}

/// The guarded update matched nothing: find out why, and clean up if the
/// project is gone.
///
/// Deletion takes the same git lock this job still holds, so it can only have
/// happened before the job started or while it was waiting for the lock;
/// either way the row's disappearance is the answer and the directories the
/// job created have to go with it (`ARCHITECTURE.md`, "Storage"). A row that
/// is still there with another status is somebody else's doing and is left
/// alone.
async fn discard(state: &AppState, project_id: Uuid) {
    match ProjectRepository::new(&state.pool).find(project_id).await {
        Ok(None) => {}
        Ok(Some(project)) => {
            warn!(
                status = ?project.status,
                "the project left `cloning` while it was being cloned; the outcome is dropped"
            );
            return;
        }
        Err(error) => {
            // Removing a directory on a guess is worse than leaving one
            // behind: orphan cleanup finds it, a wrong deletion is permanent.
            error!(%error, "the project row could not be re-read; its directory is left in place");
            return;
        }
    }

    match state.config.project_layout(project_id).remove_all().await {
        Ok(()) => info!("the project was deleted while it was cloning; its directory is gone"),
        Err(error) => error!(%error, "the deleted project's directory could not be removed"),
    }
}

/// Who the credential lookup is recorded for (`docs/data-model.md`,
/// `secret_uses`).
///
/// A REST request names its user; the mirror-fetch cron and a job restarted at
/// start-up name nobody, which is [`GitActor::System`] and a row with both ids
/// null. There is no session case here: a session cannot create a project.
fn actor_for(requested_by: Option<Uuid>) -> GitActor {
    match requested_by {
        Some(user_id) => GitActor::User(user_id),
        None => GitActor::System,
    }
}

/// The `status_message` one git failure earns (`SPEC.md`, "Projects").
///
/// The three cases a user can act on get the sentence that says what to do;
/// everything else — an unreachable host, a rejected credential, a refused
/// `init` — is git's own summary, which is the only thing that can name the
/// actual problem. The variants are matched rather than the text parsed: the
/// git layer decides what happened, this decides how to say it.
fn describe(error: &GitError) -> String {
    let message = match error {
        GitError::RemoteHasNoBranches => EMPTY_REMOTE.to_string(),
        GitError::NoRemoteDefaultBranch => NO_DEFAULT_BRANCH.to_string(),
        // The only ref initialisation reports missing is the default branch,
        // requested or discovered, after the fetch (`crate::git::mirror`).
        GitError::UnknownRef(branch) => format!("default branch {branch:?} not found on remote"),
        other => other.to_string(),
    };

    sanitize(&message)
}

/// One line, no `userinfo`, bounded length.
///
/// Whitespace runs collapse to single spaces, which is what makes git's
/// multi-line stderr a summary rather than a paragraph in a table cell.
fn sanitize(message: &str) -> String {
    let single_line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let scrubbed = scrub_userinfo(&single_line);

    if scrubbed.is_empty() {
        return UNKNOWN_FAILURE.to_string();
    }

    truncate(scrubbed)
}

/// Replace the `userinfo` of every URL in `line` with `***@`.
///
/// A credential cannot reach a [`GitError`] — it travels in a file, never in
/// argv or in the stored remote URL (`ARCHITECTURE.md`, "Git model",
/// Credentials; `docs/data-model.md`, `projects`) — so this can only ever fire
/// on a URL git itself echoed back. It is here because `status_message` is
/// displayed, and one place that cannot leak a secret is worth more than one
/// proof that nothing upstream ever will (`CLAUDE.md`, rule 3).
fn scrub_userinfo(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while let Some(offset) = rest.find(SCHEME_SEPARATOR) {
        let (scheme, after) = rest.split_at(offset + SCHEME_SEPARATOR.len());
        out.push_str(scheme);

        // The authority ends at the first path, query or fragment delimiter;
        // a `@` after that is an ordinary character, as it is in a GitLab
        // subgroup path.
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let authority = &after[..end];

        match authority.rfind('@') {
            Some(at) => {
                out.push_str(REDACTED_USERINFO);
                // Everything from the host on is kept, including the rest of
                // the URL, which may contain another one.
                rest = &after[at + 1..];
            }
            None => {
                out.push_str(authority);
                rest = &after[end..];
            }
        }
    }

    out.push_str(rest);
    out
}

/// At most [`MAX_STATUS_MESSAGE`] characters, ending in an ellipsis when
/// something was dropped.
fn truncate(message: String) -> String {
    if message.chars().count() <= MAX_STATUS_MESSAGE {
        return message;
    }

    let mut truncated: String = message.chars().take(MAX_STATUS_MESSAGE - 1).collect();
    truncated.push(ELLIPSIS);
    truncated
}

/// Wait for the clone of `project_id` to finish and return the project it
/// left behind.
///
/// The integration tests' alternative to sleeping: [`run`] reports by writing
/// the row, so the row is what a test waits on. It polls rather than holding a
/// handle, so a test that called [`spawn`] and one that called [`run`] on
/// another task wait the same way, and a job that has already finished returns
/// immediately.
///
/// Test-only, behind the `integration-tests` feature, and the panics are the
/// point: a clone that never finished has no [`Project`] to return, and the
/// message names the project and the budget it exceeded.
#[cfg(feature = "integration-tests")]
pub async fn wait_for_clone(state: &AppState, project_id: Uuid, timeout: Duration) -> Project {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let project = ProjectRepository::new(&state.pool)
            .find(project_id)
            .await
            .expect("the project row is readable")
            .unwrap_or_else(|| panic!("project {project_id} disappeared while its clone ran"));

        if project.status != ProjectStatus::Cloning {
            return project;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "project {project_id} was still `cloning` after {timeout:?}"
        );

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An obviously fake PAT, in the shape a URL would carry one (rule 3).
    const FAKE_PAT: &str = "ghp_fakefakefake";

    #[test]
    fn the_two_discovery_failures_say_what_to_do_about_them() {
        assert_eq!(describe(&GitError::RemoteHasNoBranches), EMPTY_REMOTE);
        assert_eq!(
            describe(&GitError::NoRemoteDefaultBranch),
            NO_DEFAULT_BRANCH
        );
    }

    #[test]
    fn a_missing_default_branch_is_named_in_the_message() {
        assert_eq!(
            describe(&GitError::UnknownRef("release/2.0".to_string())),
            "default branch \"release/2.0\" not found on remote"
        );
    }

    #[test]
    fn a_failed_command_becomes_git_s_own_words_on_one_line() {
        let message = describe(&GitError::Command {
            args: vec!["fetch".to_string(), "--prune".to_string()],
            code: Some(128),
            stderr: "fatal: could not read from remote\nfatal: the end\n".to_string(),
        });

        assert!(
            message.contains("fatal: could not read from remote"),
            "{message}"
        );
        assert!(!message.contains('\n'), "{message}");
        assert!(message.contains("exit 128"), "{message}");
    }

    #[test]
    fn userinfo_in_a_url_never_survives() {
        let message = sanitize(&format!(
            "fatal: unable to access 'https://x-access-token:{FAKE_PAT}@github.example.invalid/o/r.git/': 403"
        ));

        assert!(!message.contains(FAKE_PAT), "{message}");
        assert!(
            message.contains("https://***@github.example.invalid/o/r.git/"),
            "{message}"
        );
    }

    #[test]
    fn a_url_without_userinfo_is_left_alone() {
        for line in [
            "fatal: repository 'https://github.example.invalid/o/r.git' not found",
            // The `@` is in the path, not the authority: a Gitea mirror URL.
            "fatal: 'https://git.example.invalid/@scope/r.git' is unreachable",
            "ssh://git@host.example.invalid/o/r.git",
        ] {
            let scrubbed = scrub_userinfo(line);
            if line.contains("ssh://git@") {
                assert_eq!(scrubbed, "ssh://***@host.example.invalid/o/r.git");
            } else {
                assert_eq!(scrubbed, line, "an untouched URL was rewritten");
            }
        }
    }

    #[test]
    fn a_long_message_is_cut_to_the_documented_length() {
        let message = sanitize(&"x".repeat(MAX_STATUS_MESSAGE * 3));

        assert_eq!(message.chars().count(), MAX_STATUS_MESSAGE);
        assert!(message.ends_with(ELLIPSIS), "{message}");
    }

    #[test]
    fn a_message_of_nothing_at_all_still_says_something() {
        assert_eq!(sanitize("   \n\t "), UNKNOWN_FAILURE);
    }

    #[test]
    fn the_actor_is_the_user_who_asked_or_nobody() {
        let user = Uuid::new_v4();

        assert_eq!(actor_for(Some(user)), GitActor::User(user));
        assert_eq!(actor_for(None), GitActor::System);
    }
}
