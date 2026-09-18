//! Integration operations: merging one ref into an integration head, and
//! rebasing one onto another.
//!
//! `ARCHITECTURE.md`, "Git model" (Merge, rebase, push) is the contract, and
//! every step of it is here rather than in a route: the operation runs in a
//! [`TempClone`], fetches the selected source and target refs explicitly into
//! `refs/tmp/*` instead of assuming a clone copied them, merges under the bot
//! identity with a `Requested-By` trailer (Commit identity), and on success
//! writes back **only** the target integration head with one explicit
//! refspec. A conflict aborts, deletes the clone and hands the caller the
//! conflicting paths; the project repository is never left conflicted (ADR
//! 0007).
//!
//! **Merge and rebase differ in three places** and share everything else.
//! [`rebase`] rewrites the branch it was given rather than a separate target,
//! so its write-back is forced: the new commits are new objects and the old
//! tip is not their ancestor. It sets the committer only
//! ([`GitCommand::committer`]) rather than the whole identity, so each
//! replayed commit keeps its author and gains Mars as the party that
//! re-committed it. And when the rewritten branch is a session's, it
//! afterwards reconciles that session's work clone, which is the one thing in
//! this module that touches a directory an agent writes ([`WorkTreeOutcome`],
//! ADR 0019).
//!
//! **What may be merged into what.** Only an integration head is a mutation
//! target: `refs/remotes/origin/*` is upstream's, `refs/tags/*` is upstream's
//! too, `refs/handoffs/*` is immutable and a session ref is the session's
//! (ADR 0017; `SPEC.md`, "Git": upstream refs as targets are 400). A source
//! may be anything [`GitRef::is_merge_source`] admits, which is how
//! `origin/main` into `main` is the documented way to integrate fetched
//! upstream changes, and how a task merge passes a hand-off's pinned commit
//! (ADR 0018).
//!
//! **No upstream, no credential.** Everything here happens between the
//! project repository and a clone of it on the same disk. Nothing in this
//! module opens a network connection or is given a [`GitCredential`], which is
//! why none of its commands takes a
//! [`config_global`](GitCommand::config_global) (`CLAUDE.md` rule 3).
//!
//! **Locking.** The caller holds the project git lock and passes the guard,
//! and holds it from the resolution of `source` and `target` through the
//! write-back (`ARCHITECTURE.md`, "Git model", Serialization). That is what
//! makes the non-forced write-back an invariant rather than a race: the target
//! cannot have moved since it was resolved, so a rejected push is an internal
//! fault and not the caller's [`GitError::NonFastForward`].
//!
//! **Syncing.** A session source is fetched back by the service before this
//! runs (`SPEC.md`, "Git": "every session ref involved is synced first"). This
//! primitive never syncs anything itself; it merges the commits it was handed.

use std::path::Path;

use uuid::Uuid;

use super::refs::{GitRef, ResolvedRef};
use super::tempclone::TempClone;
use super::{
    CommitIdentity, DataPaths, GitActor, GitCommand, GitError, ProjectGitGuard, refs,
    session_branch,
};
use crate::prelude::*;

/// The integration-head namespace, for the two refspecs below.
const HEADS: &str = "refs/heads/";

/// The upstream-tracking namespace, which a rebase `onto` may name
/// (`SPEC.md`, "Git"). Spelled out here for the same reason [`HEADS`] is: a
/// refspec needs the fully qualified name and a [`GitRef`] carries the short
/// one.
const UPSTREAM: &str = "refs/remotes/origin/";

/// Where the target ref is fetched to inside the temporary clone.
///
/// `refs/tmp/*` and not `refs/heads/*`: the clone's branch namespace stays
/// empty apart from [`WORK_BRANCH`], so nothing about the operation depends on
/// what a clone happened to copy, and the write-back refspec can name its
/// source by hand.
const TMP_TARGET: &str = "refs/tmp/target";

/// Where the source ref is fetched to, when the source is a ref at all.
const TMP_SOURCE: &str = "refs/tmp/source";

/// Where a rebase fetches the branch it is about to rewrite.
///
/// Named for the rebase's own vocabulary rather than reusing [`TMP_TARGET`]:
/// a merge's target is the ref that gains a commit and keeps its history,
/// while a rebase's `branch` is the ref whose history is replaced, and the two
/// read as different things in a refspec.
const TMP_BRANCH: &str = "refs/tmp/branch";

/// Where a rebase fetches the ref it replays onto.
const TMP_ONTO: &str = "refs/tmp/onto";

/// The branch the merge or rebase is performed on inside the temporary clone.
/// It is created at the merge target (or the rebased branch) and pushed back
/// to it.
const WORK_BRANCH: &str = "work";

/// The trailer key that records who asked for an orchestrator-made commit
/// (`ARCHITECTURE.md`, "Git model", Commit identity).
const TRAILER_KEY: &str = "Requested-By";

/// What a completed merge produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// The commit the target integration head now points at. The merge commit
    /// when one was needed, the source commit for a fast-forward, and the
    /// unchanged target when there was nothing to merge.
    pub commit: String,
    /// Whether the target simply moved to the source commit rather than
    /// gaining a merge commit.
    ///
    /// Informational: it is the detail the `git` outcome event carries
    /// (`SPEC.md`, "Events"). An already-up-to-date merge is not a
    /// fast-forward — nothing moved.
    pub fast_forward: bool,
}

/// Merge `source` into the integration head `target`.
///
/// Both are [`ResolvedRef`]s the caller resolved under the same lock it now
/// holds, so this function works with fixed commits and never re-reads a ref
/// that could have moved. `message` is the caller's commit message, `None`
/// meaning the default `Merge <source> into <target>`; either way the
/// `Requested-By` trailer naming `requested_by` is appended, and `identity` —
/// the bot identity from `GitCredentialProvider::commit_identity` — is what
/// any commit this makes is authored and committed by.
///
/// The work happens in a temporary clone that is deleted on every exit path,
/// including a panic ([`TempClone`]). Nothing is written to the project
/// repository unless the merge succeeded and actually moved the target, and
/// what is written is one ref.
///
/// # Errors
///
/// - [`GitError::InvalidRef`] — `target` is not an integration head, or
///   `source` is a kind that may not be merged (a tag). Both are 400
///   (`SPEC.md`, "Git").
/// - [`GitError::Conflict`] — the merge stopped on conflicting paths. The
///   merge was aborted, the clone deleted and the target left exactly as it
///   was; the paths are the 422 body's `conflicts`.
/// - [`GitError::Command`] — anything else git refused, the rejected
///   write-back included: under the project git lock the target cannot have
///   moved, so a rejection is an invariant failure rather than the caller's
///   [`GitError::NonFastForward`].
pub async fn merge(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    source: &ResolvedRef,
    target: &ResolvedRef,
    message: Option<&str>,
    identity: &CommitIdentity,
    requested_by: &GitActor,
) -> std::result::Result<MergeOutcome, GitError> {
    // Only integration heads are mutation targets (ADR 0017). Destructured
    // rather than checked, because the name is what both refspecs below need.
    let GitRef::Head(target_branch) = &target.git_ref else {
        return Err(GitError::InvalidRef(target.git_ref.api_name()));
    };
    if !source.git_ref.is_merge_source() {
        return Err(GitError::InvalidRef(source.git_ref.api_name()));
    }

    let repo = paths.project_repo(guard.project_id());
    let target_full = format!("{HEADS}{target_branch}");

    // Read in the project repository, which has both commits, and before the
    // clone exists: it is only used to describe the outcome, and asking
    // afterwards would mean asking about a merge that has already happened.
    let target_is_ancestor = is_ancestor(&repo, &target.commit, &source.commit).await?;

    let clone = TempClone::create(paths, &repo).await?;
    let work = clone.path();

    // The explicit fetch `ARCHITECTURE.md` requires: an ordinary clone brings
    // `refs/heads/*` in as `refs/remotes/origin/*` and brings neither
    // `refs/sessions/*`, `refs/remotes/origin/*` nor `refs/handoffs/*` at all,
    // so the operation names what it wants instead of hoping. A commit-id
    // source is the one case with nothing to fetch: `--shared` makes every
    // object in the mirror readable here, ref or no ref.
    let mut fetch = GitCommand::new()
        .args(["fetch", "--quiet", "--end-of-options", "origin"])
        .arg(format!("+{target_full}:{TMP_TARGET}"));

    let merge_source = match source.git_ref.full_name() {
        Some(source_full) => {
            fetch = fetch.arg(format!("+{source_full}:{TMP_SOURCE}"));
            TMP_SOURCE.to_string()
        }
        None => source.commit.clone(),
    };

    fetch.cwd(work).run_ok().await?;

    // `--no-checkout` left no work tree; this creates one at the target, which
    // is what a merge needs an index and a work tree for.
    GitCommand::new()
        .args(["checkout", "--quiet", "-B", WORK_BRANCH])
        .args(["--end-of-options", TMP_TARGET])
        .cwd(work)
        .run_ok()
        .await?;

    // Built once and reused as the error's argv, so a failure reports exactly
    // what was run. A fast-forward is allowed: git makes a merge commit only
    // when the histories actually diverged.
    let merge_args = vec![
        "merge".to_string(),
        "--no-edit".to_string(),
        "--quiet".to_string(),
        "-m".to_string(),
        merge_message(source, target, message, requested_by),
        "--end-of-options".to_string(),
        merge_source,
    ];

    let output = GitCommand::new()
        .args(&merge_args)
        .identity(identity)
        .cwd(work)
        .run()
        .await?;

    if output.status != 0 {
        return Err(stopped_on_conflict(work, "merge", merge_args, &output).await);
    }

    let merged = refs::resolve(work, &GitRef::Head(WORK_BRANCH.to_string()))
        .await?
        .commit;

    // "Already up to date": the source was already reachable from the target,
    // so nothing moved. Detected by the tip, never by reading git's prose.
    if merged == target.commit {
        debug!(
            project_id = %guard.project_id(),
            git.source = %source.git_ref.api_name(),
            git.target = %target.git_ref.api_name(),
            "the merge source was already merged; the target is unchanged"
        );

        return Ok(MergeOutcome {
            commit: target.commit.clone(),
            fast_forward: false,
        });
    }

    // The write-back: one explicit refspec, non-forced, to the mirror. Git
    // validates the fast-forward for us, and the project git lock is what
    // guarantees it can only fail if something outside the lock wrote the ref.
    GitCommand::new()
        .args(["push", "--quiet", "--end-of-options", "origin"])
        .arg(format!("{WORK_BRANCH}:{target_full}"))
        .cwd(work)
        .run_ok()
        .await?;

    let fast_forward = merged == source.commit && target_is_ancestor;

    info!(
        project_id = %guard.project_id(),
        git.source = %source.git_ref.api_name(),
        git.target = %target.git_ref.api_name(),
        commit = %merged,
        git.fast_forward = fast_forward,
        "merged into an integration head"
    );

    Ok(MergeOutcome {
        commit: merged,
        fast_forward,
    })
}

/// What a completed rebase produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseOutcome {
    /// The commit the rewritten branch now points at. The replayed tip, or the
    /// unchanged tip when the branch was already based on `onto`.
    pub commit: String,
    /// What became of the session's checkout, when the branch was a session's.
    pub work_tree: WorkTreeOutcome,
}

/// What a rebase did to the session work clone behind the branch it rewrote.
///
/// Carried in the `git` outcome event's `detail` as `work_tree`
/// (`SPEC.md`, "Events"), which is why it serializes in snake case: the
/// session panel is the only place a reader learns that the checkout still
/// holds the pre-rebase commits, because a transcript event alone delivers no
/// input to the CLI (`ARCHITECTURE.md`, "Git model", Merge, rebase, push).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkTreeOutcome {
    /// The checkout was clean and on the session branch, and now holds the
    /// rewritten commits.
    Updated,
    /// The checkout was dirty, on another branch, or could not be updated. It
    /// still holds the pre-rebase commits and somebody has to reconcile it;
    /// the mirror's ref was rewritten regardless.
    ReconciliationRequired,
    /// There was no checkout to update: the branch was an integration head, or
    /// the session's work directory is gone.
    NotApplicable,
}

/// Rebase `branch` onto `onto`, rewriting `branch`.
///
/// The steps are `ARCHITECTURE.md`, "Git model" (Merge, rebase, push), and
/// they are [`merge`]'s with three differences. Both refs are resolved by the
/// caller under the lock it still holds and fetched explicitly into
/// `refs/tmp/*` in a [`TempClone`]; `git rebase` then replays `branch` onto
/// `onto` with `identity` as the **committer only**, so every replayed commit
/// keeps the author who wrote it (Commit identity); and the write-back is one
/// explicit *forced* refspec, because a rewritten history is new objects whose
/// old tip is not their ancestor. That force is not a race: the project git
/// lock is what guarantees nothing else moved `branch` since it was resolved.
///
/// `--no-autosquash` and `--no-autostash` are passed explicitly rather than
/// left to configuration. A `fixup!` subject an agent wrote must not silently
/// rewrite a different commit, and there is nothing in a freshly checked-out
/// temporary clone to stash.
///
/// A session branch's checkout is reconciled afterwards
/// ([`WorkTreeOutcome`]), and a failure to reconcile never undoes the
/// write-back: the mirror is authoritative and the checkout is the copy.
///
/// **ADR 0019.** The reconciliation commands run against the session's work
/// clone, whose git metadata the agent can write, which is the accepted,
/// documented v1 exposure (`ARCHITECTURE.md`, "Known v1 vulnerability"). The
/// status check is also a best-effort snapshot: the project git lock does not
/// cover a running container, so an agent may commit between the check and the
/// reset.
///
/// # Errors
///
/// - [`GitError::InvalidRef`] — `branch` is not an integration head or a
///   session ref, or `onto` is not an integration head or an
///   upstream-tracking ref. An upstream ref may be `onto` and never the branch
///   being rewritten (`SPEC.md`, "Git": 400).
/// - [`GitError::Conflict`] — a replayed commit stopped on conflicting paths.
///   The rebase was aborted, the clone deleted and `branch` left exactly as it
///   was; the paths are the 422 body's `conflicts`.
/// - [`GitError::Command`] — anything else git refused.
pub async fn rebase(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    branch: &ResolvedRef,
    onto: &ResolvedRef,
    identity: &CommitIdentity,
) -> std::result::Result<RebaseOutcome, GitError> {
    // Destructured rather than checked with a predicate, because the fully
    // qualified name is what both refspecs below need and only these kinds
    // have one that this operation may use (ADR 0017).
    let branch_full = match &branch.git_ref {
        GitRef::Head(name) => format!("{HEADS}{name}"),
        GitRef::Session(session_id) => refs::session_ref(*session_id),
        _ => return Err(GitError::InvalidRef(branch.git_ref.api_name())),
    };
    let onto_full = match &onto.git_ref {
        GitRef::Head(name) => format!("{HEADS}{name}"),
        GitRef::Upstream(name) => format!("{UPSTREAM}{name}"),
        _ => return Err(GitError::InvalidRef(onto.git_ref.api_name())),
    };

    let repo = paths.project_repo(guard.project_id());
    let clone = TempClone::create(paths, &repo).await?;
    let work = clone.path();

    // The explicit fetch the document requires: a clone brings neither
    // `refs/sessions/*` nor the mirror's own `refs/remotes/origin/*`, so both
    // ends of the rebase are named here rather than assumed.
    GitCommand::new()
        .args(["fetch", "--quiet", "--end-of-options", "origin"])
        .arg(format!("+{branch_full}:{TMP_BRANCH}"))
        .arg(format!("+{onto_full}:{TMP_ONTO}"))
        .cwd(work)
        .run_ok()
        .await?;

    // `--no-checkout` left no work tree; the rebase needs one.
    GitCommand::new()
        .args(["checkout", "--quiet", "-B", WORK_BRANCH])
        .args(["--end-of-options", TMP_BRANCH])
        .cwd(work)
        .run_ok()
        .await?;

    // Built once and reused as the error's argv, so a failure reports exactly
    // what was run.
    let rebase_args = vec![
        "rebase".to_string(),
        "--quiet".to_string(),
        "--no-autosquash".to_string(),
        "--no-autostash".to_string(),
        "--end-of-options".to_string(),
        TMP_ONTO.to_string(),
    ];

    let output = GitCommand::new()
        .args(&rebase_args)
        .committer(identity)
        .cwd(work)
        .run()
        .await?;

    if output.status != 0 {
        return Err(stopped_on_conflict(work, "rebase", rebase_args, &output).await);
    }

    // The outcome is the resulting tip, never git's prose: an already-based
    // branch, a fast-forward to `onto` and a full replay are all one answer
    // here, and the caller only needs the commit the ref now carries.
    let rebased = refs::resolve(work, &GitRef::Head(WORK_BRANCH.to_string()))
        .await?
        .commit;

    // Forced, and unconditionally: a rebase that changed nothing pushes the
    // value the ref already has, which git accepts as the no-op it is, and
    // skipping it would only add a branch nothing else needs.
    GitCommand::new()
        .args(["push", "--quiet", "--end-of-options", "origin"])
        .arg(format!("+{WORK_BRANCH}:{branch_full}"))
        .cwd(work)
        .run_ok()
        .await?;

    let work_tree = match &branch.git_ref {
        GitRef::Session(session_id) => reconcile_work_clone(paths, *session_id).await,
        _ => WorkTreeOutcome::NotApplicable,
    };

    info!(
        project_id = %guard.project_id(),
        git.branch = %branch.git_ref.api_name(),
        git.onto = %onto.git_ref.api_name(),
        commit = %rebased,
        git.work_tree = ?work_tree,
        "rebased a branch"
    );

    Ok(RebaseOutcome {
        commit: rebased,
        work_tree,
    })
}

/// Bring a session's checkout onto the rewritten `refs/sessions/<sid>`, when
/// it is safe to do so.
///
/// Never fails: the mirror has already been written and the caller's rebase
/// succeeded, so the worst this can report is that somebody has to reconcile
/// the checkout by hand. A git failure here is therefore a `warn!` and a
/// [`WorkTreeOutcome::ReconciliationRequired`], not an error that would make a
/// successful rebase look like a failed one.
async fn reconcile_work_clone(paths: &DataPaths, session_id: Uuid) -> WorkTreeOutcome {
    let work = paths.session_work(session_id);

    // No checkout to update: the session was deleted, or never had one. Not a
    // problem to report, so not [`WorkTreeOutcome::ReconciliationRequired`].
    if !work.exists() {
        return WorkTreeOutcome::NotApplicable;
    }

    match reconcile(&work, session_id).await {
        Ok(outcome) => outcome,
        Err(err) => {
            warn!(
                session_id = %session_id,
                error = %err,
                "the session work clone could not be reconciled after a rebase; \
                 the rewritten ref stands and the checkout needs reconciling"
            );
            WorkTreeOutcome::ReconciliationRequired
        }
    }
}

/// The three commands reconciliation is made of, as a fallible unit.
///
/// `fetch` first, because the work clone's `origin` is the mirror and the
/// rewritten commits are only there; then the two questions that decide
/// whether a `reset --hard` would destroy anything, and only then the reset.
///
/// "Clean" is `--untracked-files=no` on purpose: an untracked file survives
/// `git reset --hard` untouched, so it is not work the reset would discard,
/// and refusing to reconcile over a stray build artefact would leave every
/// checkout behind (`ARCHITECTURE.md`, "Git model", Merge, rebase, push).
async fn reconcile(
    work: &Path,
    session_id: Uuid,
) -> std::result::Result<WorkTreeOutcome, GitError> {
    let session_full = refs::session_ref(session_id);

    GitCommand::new()
        .args(["fetch", "--quiet", "--end-of-options", "origin"])
        .arg(format!("+{session_full}:{session_full}"))
        .cwd(work)
        .run_ok()
        .await?;

    let status = GitCommand::new()
        .args(["status", "--porcelain", "--untracked-files=no"])
        .cwd(work)
        .run_ok()
        .await?;

    // A detached HEAD is not a failure — the agent checked something else out
    // — so a non-zero exit is the answer here and `--quiet` keeps git from
    // printing about it.
    let head = GitCommand::new()
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .cwd(work)
        .run()
        .await?;
    let on_session_branch =
        head.status == 0 && head.stdout.trim() == format!("{HEADS}{}", session_branch(session_id));

    if !status.stdout.trim().is_empty() || !on_session_branch {
        debug!(
            session_id = %session_id,
            git.on_session_branch = on_session_branch,
            "the session work clone was not in a state a reset could be forced on"
        );
        return Ok(WorkTreeOutcome::ReconciliationRequired);
    }

    GitCommand::new()
        .args(["reset", "--hard", "--quiet", "--end-of-options"])
        .arg(&session_full)
        .cwd(work)
        .run_ok()
        .await?;

    debug!(
        session_id = %session_id,
        "the session work clone was reset onto the rewritten session ref"
    );

    Ok(WorkTreeOutcome::Updated)
}

/// The `Requested-By: …` line naming who asked for a commit the orchestrator
/// makes (`ARCHITECTURE.md`, "Git model", Commit identity).
///
/// One line, no trailing newline: the callers that build a message put it
/// after a blank line, which is what makes it a trailer git itself can read
/// back with `git interpret-trailers --parse`.
pub fn requested_by_trailer(actor: &GitActor) -> String {
    match actor {
        GitActor::User(user_id) => format!("{TRAILER_KEY}: user:{user_id}"),
        GitActor::Session(session_id) => format!("{TRAILER_KEY}: session:{session_id}"),
        GitActor::System => format!("{TRAILER_KEY}: system"),
    }
}

/// The paths a stopped merge or rebase left conflicting, as git names them.
///
/// `git diff --name-only --diff-filter=U -z`: the unmerged entries of the
/// index, which is where a modify/delete conflict and a binary conflict appear
/// like any other path. `-z` because git would otherwise quote and escape a
/// path with unusual characters in it, and these go straight into a JSON
/// response (`SPEC.md`, "REST API": `conflicts`).
pub(crate) async fn conflicting_paths(clone: &Path) -> std::result::Result<Vec<String>, GitError> {
    let output = GitCommand::new()
        .args(["diff", "--name-only", "--diff-filter=U", "-z"])
        .cwd(clone)
        .run_ok()
        .await?;

    Ok(output
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

/// Turn a non-zero `git merge` or `git rebase` into the error it stands for.
///
/// Conflicting paths make it a [`GitError::Conflict`] and `git <subcommand>
/// --abort` is run first; anything else — a bad argument, a broken object — is
/// a [`GitError::Command`] carrying the argv as run. A failure while asking
/// about the conflict is itself the answer, so it is returned as it is.
///
/// `subcommand` is `merge` or `rebase`: the two have separate abort commands
/// and each only knows how to abort its own kind of stopped operation.
async fn stopped_on_conflict(
    clone: &Path,
    subcommand: &str,
    args: Vec<String>,
    output: &super::GitOutput,
) -> GitError {
    let paths = match conflicting_paths(clone).await {
        Ok(paths) => paths,
        Err(err) => return err,
    };

    if paths.is_empty() {
        return GitError::Command {
            args,
            // `run` reports a signal or a timeout as a negative status; every
            // real exit code is non-negative.
            code: (output.status >= 0).then_some(output.status),
            stderr: output.stderr.clone(),
        };
    }

    // The clone is deleted either way when it drops; aborting leaves it in a
    // sane state in the one case deletion fails, and costs one local command.
    let aborted = GitCommand::new()
        .args([subcommand, "--abort"])
        .cwd(clone)
        .run()
        .await;
    if !matches!(&aborted, Ok(output) if output.status == 0) {
        debug!(
            path = %clone.display(),
            git.subcommand = %subcommand,
            "the conflicting operation could not be aborted; the clone is deleted anyway"
        );
    }

    GitError::Conflict { paths }
}

/// The message a merge commit is made with.
///
/// The caller's text or the documented default, then a blank line, then the
/// `Requested-By` trailer — the formatting `git interpret-trailers` would
/// produce. A message that already ends in a trailer block still gets one
/// appended; git reads several trailers happily, and dropping the attribution
/// would be worse than repeating a key. A message that is only whitespace is
/// treated as no message: a commit whose subject is the trailer would be
/// unreadable.
///
/// Length is not limited here. `POST /projects/{pid}/git/merge` rejects an
/// over-long message with 400 before it reaches this layer.
fn merge_message(
    source: &ResolvedRef,
    target: &ResolvedRef,
    message: Option<&str>,
    requested_by: &GitActor,
) -> String {
    let body = message
        .map(str::trim_end)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "Merge {} into {}",
                source.git_ref.api_name(),
                target.git_ref.api_name()
            )
        });

    format!("{body}\n\n{}\n", requested_by_trailer(requested_by))
}

/// Is `ancestor` reachable from `descendant`?
///
/// `git merge-base --is-ancestor`, whose exit code is the answer: 0 yes, 1 no,
/// anything else a failure. This is what separates a fast-forward from a real
/// merge without parsing what `git merge` printed.
async fn is_ancestor(
    repo: &Path,
    ancestor: &str,
    descendant: &str,
) -> std::result::Result<bool, GitError> {
    let args = vec![
        "merge-base".to_string(),
        "--is-ancestor".to_string(),
        "--end-of-options".to_string(),
        ancestor.to_string(),
        descendant.to_string(),
    ];

    let output = GitCommand::new().args(&args).cwd(repo).run().await?;

    match output.status {
        0 => Ok(true),
        1 => Ok(false),
        status => Err(GitError::Command {
            args,
            code: (status >= 0).then_some(status),
            stderr: output.stderr,
        }),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    /// A resolved ref with a plausible-looking commit, for the message tests.
    fn resolved(git_ref: GitRef) -> ResolvedRef {
        ResolvedRef {
            git_ref,
            commit: "a".repeat(40),
        }
    }

    #[test]
    fn the_trailer_names_each_kind_of_actor() {
        let user_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        assert_eq!(
            requested_by_trailer(&GitActor::User(user_id)),
            format!("Requested-By: user:{user_id}")
        );
        assert_eq!(
            requested_by_trailer(&GitActor::Session(session_id)),
            format!("Requested-By: session:{session_id}")
        );
        assert_eq!(
            requested_by_trailer(&GitActor::System),
            "Requested-By: system"
        );
    }

    #[test]
    fn the_default_message_names_both_api_names_and_carries_the_trailer() {
        let session_id = Uuid::new_v4();
        let message = merge_message(
            &resolved(GitRef::Session(session_id)),
            &resolved(GitRef::Head("main".to_string())),
            None,
            &GitActor::User(Uuid::nil()),
        );

        assert_eq!(
            message,
            format!(
                "Merge {session_id} into main\n\nRequested-By: user:{}\n",
                Uuid::nil()
            )
        );
    }

    #[test]
    fn an_upstream_source_keeps_its_origin_prefix_in_the_default_message() {
        let message = merge_message(
            &resolved(GitRef::Upstream("main".to_string())),
            &resolved(GitRef::Head("main".to_string())),
            None,
            &GitActor::System,
        );

        assert!(
            message.starts_with("Merge origin/main into main\n\n"),
            "{message}"
        );
    }

    #[test]
    fn a_supplied_message_keeps_its_text_and_gains_a_trailer_after_a_blank_line() {
        let message = merge_message(
            &resolved(GitRef::Head("feature/x".to_string())),
            &resolved(GitRef::Head("main".to_string())),
            Some("feat: the thing\n\nWith a body.\n"),
            &GitActor::System,
        );

        assert_eq!(
            message,
            "feat: the thing\n\nWith a body.\n\nRequested-By: system\n"
        );
    }

    #[test]
    fn a_message_that_already_has_a_trailer_still_gains_this_one() {
        let message = merge_message(
            &resolved(GitRef::Head("feature/x".to_string())),
            &resolved(GitRef::Head("main".to_string())),
            Some("fix: it\n\nReviewed-By: somebody"),
            &GitActor::System,
        );

        assert_eq!(
            message,
            "fix: it\n\nReviewed-By: somebody\n\nRequested-By: system\n"
        );
    }

    #[test]
    fn a_blank_message_falls_back_to_the_default() {
        let message = merge_message(
            &resolved(GitRef::Head("feature/x".to_string())),
            &resolved(GitRef::Head("main".to_string())),
            Some("   \n"),
            &GitActor::System,
        );

        assert!(
            message.starts_with("Merge feature/x into main\n\n"),
            "{message}"
        );
    }
}
