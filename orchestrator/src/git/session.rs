//! The session side of the git model: base resolution, the work clone and
//! fetch-back.
//!
//! `ARCHITECTURE.md`, "Git model" (Session clone, Fetch-back) is the contract.
//! A launch resolves `base_ref` to a commit in the project repository
//! ([`resolve_base`]), creates `DATA_DIR/sessions/<sid>/work` as a reference
//! clone of that repository and puts `session/<sid>` at the resolved commit
//! ([`create_work_clone`]). Everything that later needs the session's work in
//! the project repository — `POST /sessions/{id}/end`, `POST
//! /sessions/{id}/sync`, hand-off publication, merge, rebase, push and the
//! diff endpoint — calls [`fetch_back`] first, or [`fetch_back_ended`] for a
//! session that has ended or is ending, which keeps the ref only when the
//! session made commits beyond its base (ADR 0050). Session deletion calls
//! [`remove_work_clone`] (`ARCHITECTURE.md`, "Storage").
//!
//! **What the clone borrows.** The work clone holds no objects of its own:
//! `--reference` records the mirror in `.git/objects/info/alternates`, so a
//! session costs its checkout plus whatever the agent commits (ADR 0001). The
//! mirror is mounted read-only into the container at the orchestrator's own
//! path, which is why the alternates entry is built from [`DataPaths`]
//! (`DATA_DIR`) and never from `DATA_DIR_HOST`.
//!
//! **What an ordinary clone does not copy.** `refs/heads/*` arrive as
//! `refs/remotes/origin/*` and `refs/tags/*` as themselves; the mirror's own
//! `refs/remotes/origin/*`, `refs/sessions/*` and `refs/handoffs/*` do not
//! arrive at all. A base in one of those namespaces is therefore fetched by
//! its fully qualified name before the checkout, exactly as the document says.
//! A commit-id base needs no fetch: the object is reachable through the
//! alternates.
//!
//! **Credentials.** Nothing here touches upstream, so no command in this
//! module is given a credential and the clone is left with none: its `origin`
//! is the mirror's path and it has no `http.extraHeader` and no
//! `GIT_CONFIG_GLOBAL` of its own (ADR 0007; `CLAUDE.md` rule 3). The agent
//! can `git fetch origin` inside its container and cannot push anywhere.
//!
//! **Locking.** The caller holds the project git lock and passes the guard:
//! a fresh launch holds it through base resolution and clone setup, and
//! fetch-back runs under it as well (`ARCHITECTURE.md`, "Git model",
//! Serialization). [`remove_work_clone`] is the exception and takes no guard —
//! it deletes a session's own directory and never touches the mirror.
//!
//! **ADR 0019.** The work clone's git metadata is agent-controlled, and
//! `fetch` reads it. A fetch *from* a path executes none of that repository's
//! hooks, but running git against an agent-controlled checkout at all remains
//! the accepted, documented v1 exposure.

use std::path::Path;

use uuid::Uuid;

use super::refs::{GitRef, ResolvedRef};
use super::{CommitIdentity, DataPaths, GitCommand, GitError, ProjectGitGuard, refs};
use crate::prelude::*;

/// The integration-head namespace, for qualifying a bare `default_branch`.
const HEADS: &str = "refs/heads/";

/// The branch a session's work lives on inside its clone
/// (`docs/data-model.md`, `sessions.branch`).
///
/// Not the same thing as [`refs::session_ref`], which is where fetch-back puts
/// that branch in the project repository.
pub fn session_branch(session_id: Uuid) -> String {
    format!("session/{session_id}")
}

/// Resolve a launch's `base_ref` to a commit in the project repository.
///
/// `None` is the project default: the Mars integration head named by
/// `default_branch` (`SPEC.md`, "Projects"). A `Some` value is whatever the
/// caller sent, parsed by [`GitRef::parse`], so an integration head, an
/// upstream-tracking ref, a tag, a session ref, a fully qualified hand-off ref
/// or a commit id all reach the same resolution. A bare tag name works through
/// [`refs::resolve`]'s head-then-tag fallback.
///
/// Hand-off bases are accepted only fully qualified as `refs/handoffs/<id>`,
/// because a bare UUID is a session ref. The code hand-offs epic does not rely
/// on that spelling: it passes the hand-off's pinned commit as a 40-character
/// object id (`docs/data-model.md`, `task_handoffs`).
///
/// # Errors
///
/// - [`GitError::InvalidRef`] — the name is not a usable ref name.
/// - [`GitError::UnknownRef`] — nothing in the project repository resolves to
///   it.
/// - [`GitError::NotACommit`] — it resolves to an object that is not a commit.
///
/// All three are 400 at `POST /projects/{pid}/sessions` (`SPEC.md`,
/// "Sessions"; [`GitError::status`]).
pub async fn resolve_base(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    base_ref: Option<&str>,
    default_branch: &str,
) -> std::result::Result<ResolvedRef, GitError> {
    let repo = paths.project_repo(guard.project_id());

    let requested = match base_ref {
        Some(raw) => GitRef::parse(raw)?,
        // Qualified before parsing rather than built as `GitRef::Head`
        // directly, so a stored `default_branch` that is not a usable branch
        // name is refused here instead of reaching argv — the same treatment
        // `mirror::init_project_repo` gives a requested default.
        None => GitRef::parse(&format!("{HEADS}{default_branch}"))?,
    };

    let resolved = refs::resolve(&repo, &requested).await?;

    debug!(
        project_id = %guard.project_id(),
        git.base = %resolved.git_ref.api_name(),
        commit = %resolved.commit,
        "resolved a session base ref"
    );

    Ok(resolved)
}

/// Create `DATA_DIR/sessions/<session_id>/work` at `base`, on branch
/// `session/<session_id>`.
///
/// The steps are `ARCHITECTURE.md`, "Git model" (Session clone): clone the
/// project repository against itself as the reference, fetch the selected ref
/// when it is one an ordinary clone does not copy, create the session branch
/// at the resolved commit, and configure the launching user's identity so the
/// agent's commits are attributed to them (Commit identity).
///
/// Two flags are added to the documented command line, and neither changes
/// what it produces:
///
/// - `--shared`, because `--reference` on a local path does *not* stop
///   `git clone` copying the source's whole object directory; without it every
///   session would duplicate the project's history and the alternates file
///   would be decoration (ADR 0001, "disk cost per session is the checkout
///   plus new objects only"). It is the same mechanism `ARCHITECTURE.md` uses
///   for the merge and rebase temporary clones, and it adds no second
///   alternates entry: the reference is already that path.
/// - `--no-hardlinks`, so that if an object file were ever copied into the
///   clone after all it would be a copy, and the work tree — which the agent
///   writes — can never share an inode with the mirror.
///
/// Called for a fresh launch only. Any `work` directory already there is
/// removed first: a relaunch after a failed `creating` finds whatever the
/// interrupted attempt left, which is not a clone anybody can use.
///
/// The caller holds the project git lock; the work clone itself is not a lock
/// subject afterwards, because it belongs to one session.
pub async fn create_work_clone(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    session_id: Uuid,
    base: &ResolvedRef,
    identity: &CommitIdentity,
) -> std::result::Result<(), GitError> {
    let repo = paths.project_repo(guard.project_id());
    let session_dir = paths.session_dir(session_id);
    let work = paths.session_work(session_id);

    remove_work_clone(paths, session_id)?;
    std::fs::create_dir_all(&session_dir)?;

    GitCommand::new()
        .args([
            "clone",
            "--quiet",
            "--no-checkout",
            "--shared",
            "--no-hardlinks",
        ])
        .arg("--reference")
        .arg(&repo)
        .arg("--end-of-options")
        .arg(&repo)
        .arg(&work)
        .cwd(&session_dir)
        .run_ok()
        .await?;

    if let Some(full_name) = base.git_ref.full_name()
        && needs_explicit_fetch(&base.git_ref)
    {
        // Into `FETCH_HEAD` only: the checkout below names the commit, and
        // giving the clone a local ref in a namespace it does not own would
        // only invite the agent to move it.
        GitCommand::new()
            .args(["fetch", "--quiet", "--end-of-options", "origin", &full_name])
            .cwd(&work)
            .run_ok()
            .await?;
    }

    let branch = session_branch(session_id);
    // The revision before a trailing `--` rather than after
    // `--end-of-options`: `git checkout` does not understand that option on
    // git 2.39, the minimum supported (see the module docs). `base.commit`
    // is a full object id `resolve_base` obtained from git itself, so it cannot
    // start with `-`.
    GitCommand::new()
        .args(["checkout", "--quiet", "-b", &branch])
        .args([base.commit.as_str(), "--"])
        .cwd(&work)
        .run_ok()
        .await?;

    // `user.name`/`user.email` rather than `GIT_AUTHOR_*`: these commits are
    // made by the agent inside the container, where nothing this process sets
    // in an environment would reach them.
    set_config(&work, "user.name", &identity.name).await?;
    set_config(&work, "user.email", &identity.email).await?;

    info!(
        session_id = %session_id,
        project_id = %guard.project_id(),
        git.branch = %branch,
        commit = %base.commit,
        "session work clone created"
    );

    Ok(())
}

/// Copy `session/<session_id>` from the session's work clone into
/// `refs/sessions/<session_id>` in the project repository, and return the
/// commit the ref now points at.
///
/// `git fetch <work> +session/<sid>:refs/sessions/<sid>`, forced:
/// the session owns that ref, so an agent that rewrote its own history is
/// followed rather than rejected (`ARCHITECTURE.md`, "Git model", Fetch-back).
/// Hand-off publication is what checks a fetched tip against a requested
/// commit (ADR 0018); this function only moves the ref.
///
/// The fetch-back of a live session. One that has ended, or is ending, goes
/// through [`fetch_back_ended`] instead, which may decline to keep the ref.
///
/// Silent by design. The `git` event with `op: "sync"` belongs to the service
/// task that asked for an explicit sync, so the diff endpoint can reuse this
/// without emitting one and re-triggering the panel's refresh-on-event rule
/// (`ARCHITECTURE.md`, "Git model", Diff).
///
/// # Errors
///
/// [`GitError::UnknownRef`] `session/<sid>` when there is nothing to fetch
/// back — the work directory is gone, or the agent deleted the branch inside
/// it. The project repository's ref is left exactly as it was in both cases.
pub async fn fetch_back(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    session_id: Uuid,
) -> std::result::Result<String, GitError> {
    work_tip(paths, session_id).await?;
    fetch_into_session_ref(guard, paths, session_id).await
}

/// What a fetch-back did: [`fetch_back_ended`]'s answer, and the answer of
/// the service's sync, which a live fetch-back always gives with `kept`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedBack {
    /// The tip of `session/<sid>` in the work clone, which is also where
    /// `refs/sessions/<sid>` points when [`kept`](Self::kept) is true.
    pub commit: String,
    /// Whether `refs/sessions/<sid>` exists afterwards.
    pub kept: bool,
}

/// The fetch-back of a session that has ended or is ending: the same fetch as
/// [`fetch_back`], unless the session made no commits beyond its base.
///
/// When the work clone's `session/<sid>` tip is `base_commit` — a planner, or
/// a reviewer that forwarded a hand-off without committing — the ref would
/// hold nothing the session did, so it is not written, and one an earlier
/// sync left is deleted (`ARCHITECTURE.md`, "Git model", Ref ownership;
/// ADR 0050). The work clone is not touched: it stays until the session is
/// deleted, and a later fetch-back — of a retried session, which is live again
/// — reads it as before.
///
/// `base_commit` is `sessions.base_commit`; a session launched before that
/// column existed has none, and for it this is exactly [`fetch_back`].
///
/// # Errors
///
/// As [`fetch_back`]. The ref is deleted only after the tip was read, so a
/// work clone with nothing to read leaves the project repository as it was.
pub async fn fetch_back_ended(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    session_id: Uuid,
    base_commit: Option<&str>,
) -> std::result::Result<FetchedBack, GitError> {
    let tip = work_tip(paths, session_id).await?;

    if base_commit == Some(tip.as_str()) {
        let repo = paths.project_repo(guard.project_id());
        refs::delete(&repo, &refs::session_ref(session_id)).await?;

        debug!(
            session_id = %session_id,
            commit = %tip,
            "the ended session made no commits beyond its base; it keeps no session ref"
        );

        return Ok(FetchedBack {
            commit: tip,
            kept: false,
        });
    }

    let commit = fetch_into_session_ref(guard, paths, session_id).await?;
    Ok(FetchedBack { commit, kept: true })
}

/// The tip of `session/<sid>` in the session's work clone.
///
/// [`GitError::UnknownRef`] `session/<sid>` when there is none: the work
/// directory is gone, or the agent deleted the branch inside it.
async fn work_tip(paths: &DataPaths, session_id: Uuid) -> std::result::Result<String, GitError> {
    let work = paths.session_work(session_id);
    let branch = session_branch(session_id);

    // Checked before git runs: a command whose working directory does not
    // exist fails to spawn at all, which is an I/O fault rather than the
    // answer "this session has nothing to sync".
    if !work.exists() {
        return Err(GitError::UnknownRef(branch));
    }

    work_branch_tip(&work, &branch)
        .await?
        .ok_or(GitError::UnknownRef(branch))
}

/// The forced fetch of `session/<sid>` into `refs/sessions/<sid>`, answering
/// the commit the ref then points at. The caller has checked that the branch
/// is there.
async fn fetch_into_session_ref(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    session_id: Uuid,
) -> std::result::Result<String, GitError> {
    let repo = paths.project_repo(guard.project_id());
    let work = paths.session_work(session_id);
    let branch = session_branch(session_id);

    let refspec = format!("+{branch}:{}", refs::session_ref(session_id));
    GitCommand::new()
        .args(["fetch", "--quiet", "--end-of-options"])
        .arg(&work)
        .arg(&refspec)
        .cwd(&repo)
        .run_ok()
        .await?;

    let commit = refs::resolve(&repo, &GitRef::Session(session_id))
        .await?
        .commit;

    debug!(
        session_id = %session_id,
        commit = %commit,
        "session branch fetched back into the project repository"
    );

    Ok(commit)
}

/// Delete `DATA_DIR/sessions/<session_id>/work`.
///
/// Session deletion, which then removes the rest of the session's directory
/// (`ARCHITECTURE.md`, "Storage"). No project git lock: the mirror is not
/// touched, and the objects the clone borrowed were never its own. Idempotent,
/// because a work directory that is not there is the wanted state.
pub fn remove_work_clone(paths: &DataPaths, session_id: Uuid) -> std::result::Result<(), GitError> {
    match std::fs::remove_dir_all(paths.session_work(session_id)) {
        Ok(()) => {
            debug!(session_id = %session_id, "session work clone removed");
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(GitError::Io(err)),
    }
}

/// Does a clone of the project repository need this ref fetched explicitly
/// before the checkout?
///
/// An ordinary clone copies `refs/heads/*` (as `refs/remotes/origin/*`) and
/// `refs/tags/*`. It copies neither the source's own upstream-tracking refs
/// nor the two namespaces Mars adds, and a commit id is not a ref at all — it
/// comes from the alternates.
fn needs_explicit_fetch(git_ref: &GitRef) -> bool {
    match git_ref {
        GitRef::Upstream(_) | GitRef::Tag(_) | GitRef::Session(_) | GitRef::Handoff(_) => true,
        // A tag is in fact copied by an ordinary clone; it is listed above
        // because the document says to fetch the selected ref explicitly and a
        // clone's tag following is a default that could be configured away.
        GitRef::Head(_) | GitRef::Commit(_) => false,
    }
}

/// The commit `refs/heads/<branch>` points at in the work clone, or `None`
/// when the branch is not there.
///
/// A missing branch is the answer, not a failure, so this goes through
/// [`GitCommand::run`]. A work directory that is not a repository at all is
/// something else entirely, and the second command is what tells the two
/// apart: without it a corrupted clone would answer 400 "no such ref" instead
/// of the internal fault it is. Same shape as [`refs::resolve`]'s final check.
async fn work_branch_tip(
    work: &Path,
    branch: &str,
) -> std::result::Result<Option<String>, GitError> {
    // Peeled, so the answer is always a commit id and compares with
    // `sessions.base_commit` as a string.
    let full_name = format!("{HEADS}{branch}^{{commit}}");
    let output = GitCommand::new()
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(&full_name)
        .cwd(work)
        .run()
        .await?;

    if output.status == 0 {
        return Ok(Some(output.stdout.trim().to_string()));
    }

    GitCommand::new()
        .args(["rev-parse", "--git-dir"])
        .cwd(work)
        .run_ok()
        .await?;

    Ok(None)
}

/// One `git config --replace-all <key> <value>` in the work clone.
///
/// `--replace-all` so a relaunch that reused a directory could not leave two
/// values behind, which git reads as a multi-valued key.
async fn set_config(work: &Path, key: &str, value: &str) -> std::result::Result<(), GitError> {
    GitCommand::new()
        .args(["config", "--replace-all", "--end-of-options", key, value])
        .cwd(work)
        .run_ok()
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed id, so the expectations below can be read at a glance.
    const ID: &str = "11111111-2222-3333-4444-555555555555";

    fn id() -> Uuid {
        Uuid::parse_str(ID).expect("a fixed uuid")
    }

    #[test]
    fn the_session_branch_is_the_documented_name() {
        assert_eq!(session_branch(id()), format!("session/{ID}"));
    }

    #[test]
    fn the_branch_and_the_project_repository_ref_are_different_names() {
        assert_ne!(session_branch(id()), refs::session_ref(id()));
        assert_eq!(refs::session_ref(id()), format!("refs/sessions/{ID}"));
    }

    #[test]
    fn only_the_namespaces_a_clone_does_not_copy_are_fetched_explicitly() {
        for git_ref in [
            GitRef::Upstream("main".into()),
            GitRef::Tag("v1.0.0".into()),
            GitRef::Session(id()),
            GitRef::Handoff(id()),
        ] {
            assert!(needs_explicit_fetch(&git_ref), "{git_ref:?}");
        }

        for git_ref in [GitRef::Head("main".into()), GitRef::Commit("0".repeat(40))] {
            assert!(!needs_explicit_fetch(&git_ref), "{git_ref:?}");
        }
    }

    #[test]
    fn every_ref_that_is_fetched_explicitly_has_a_name_to_fetch() {
        // The fetch is guarded by `full_name()` as well; a kind that wanted a
        // fetch but had no ref name would silently skip it.
        for git_ref in [
            GitRef::Upstream("main".into()),
            GitRef::Tag("v1.0.0".into()),
            GitRef::Session(id()),
            GitRef::Handoff(id()),
        ] {
            assert!(git_ref.full_name().is_some(), "{git_ref:?}");
        }
    }
}
