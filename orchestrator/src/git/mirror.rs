//! Creating, configuring, fetching, listing and removing a project's bare
//! repository.
//!
//! `ARCHITECTURE.md`, "Git model" (Project clone) is the contract, step for
//! step: `git init --bare`, `origin` with the two documented fetch refspecs,
//! `gc.auto=0` and `gc.pruneExpire=never` so a session clone's borrowed
//! objects are never collected underneath it (ADR 0001), `remote.origin.mirror`
//! left unset because the repository is not an exact mirror (ADR 0017),
//! discovery of upstream's symbolic `HEAD` when the caller supplied no
//! `default_branch`, the first fetch, one Mars integration head seeded per
//! fetched upstream branch, and `HEAD` pointed at the default integration
//! branch.
//!
//! One addition to that list, and not a contract change:
//! `core.logAllRefUpdates true`. A bare repository defaults it off, so
//! `refs/heads/*` and `refs/sessions/*` would be rewritten without a reflog
//! and an operator recovering from a bad integration would have nothing to
//! read. It costs a line per ref update.
//!
//! **Idempotent by construction**, because the background clone job is
//! retryable (`SPEC.md`, "Projects": `retry-clone`): every config write
//! replaces rather than appends, `git init` on an existing repository
//! re-initialises it, the fetch is a fetch, and seeding skips every head that
//! is already there. An integration head is never moved here — that is what
//! ADR 0017 separates the namespaces for — so a re-run after upstream advanced
//! updates `refs/remotes/origin/*` and leaves Mars's branches alone.
//!
//! **The recurring fetch.** [`fetch_upstream`] is the one `git fetch --prune
//! origin` the cron mirror-fetch job, `POST /projects/{id}/fetch` and a fresh
//! session launch all run (`ARCHITECTURE.md`, "Git model", Project clone) —
//! literally the same function the first fetch of [`init_project_repo`] goes
//! through. It touches only what the two configured refspecs cover:
//! `refs/remotes/origin/*` and `refs/tags/*`. `refs/heads/*`,
//! `refs/sessions/*` and `refs/handoffs/*` are Mars's and are never arguments
//! to it, which is what makes "a fetch must preserve an unpushed merge on an
//! integration branch, even if upstream moves or deletes that branch"
//! (`ARCHITECTURE.md`, "Git model", Ref ownership; ADR 0017) a property of the
//! command rather than of a check afterwards. `--prune` therefore deletes only
//! upstream-owned refs: the tracking ref of a branch upstream dropped, and —
//! because of the tag refspec — a tag upstream dropped.
//!
//! [`fetch_project`] is that fetch with the orchestrator's context around it:
//! the readiness check, the project git lock, the credential and the
//! `projects.last_fetched_at` write. [`list_branches`] is the read side,
//! `GET /projects/{id}/branches` (`SPEC.md`, "Projects").
//!
//! **Locking.** The caller holds the project git lock and passes the guard;
//! nothing here acquires one (`ARCHITECTURE.md`, "Git model", Serialization).
//! [`fetch_project`] is the one exception, and it is a composite operation
//! rather than a helper: it takes the lock itself, holds it for exactly the
//! git command and releases it before the short database transaction, so no
//! transaction is ever open while git runs (ADR 0021).
//!
//! **Credentials.** Only the two commands that talk to the remote — the
//! symref probe and the fetch — get one, through a temporary mode-0600 config
//! ([`CredentialConfig`]) that is deleted before this function returns, on the
//! failure path as well. The URL configured as `remote.origin.url` is the
//! stored [`RemoteUrl`] and never carries one (rule 3; `ARCHITECTURE.md`,
//! "Git model", Credentials).

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{
    CredentialConfig, DataPaths, GitActor, GitCommand, GitCredential, GitError, GitOutput,
    ProjectGitGuard, refs,
};
use crate::models::{Branch, BranchKind, Project, ProjectStatus, RemoteUrl};
use crate::prelude::*;
use crate::repositories::ProjectRepository;

/// The integration-head namespace Mars owns (ADR 0017).
const HEADS: &str = "refs/heads/";
/// The upstream-tracking namespace a fetch refreshes.
const UPSTREAM: &str = "refs/remotes/origin/";

/// The pattern `for-each-ref` takes for every integration head.
///
/// A prefix and not `refs/heads/*`: a pattern with a wildcard in it is matched
/// with `wildmatch`, whose `*` stops at a `/`, so the glob form would miss
/// every branch whose name has a slash in it (`feature/x`). A pattern without
/// one matches literally or up to a slash, which is the whole subtree.
const HEADS_PATTERN: &str = "refs/heads/";
/// The same for every upstream-tracking ref. `refs/remotes/origin/HEAD` is
/// dropped by [`refs::list`]; the loop below skips it a second time, because
/// an upstream `HEAD` that is not a symref would arrive as an ordinary entry.
const UPSTREAM_PATTERN: &str = "refs/remotes/origin/";
/// The same for every session ref, the third namespace a [`Branch`] can come
/// from. Hand-off refs have no pattern here at all: they are not branches
/// (`SPEC.md`, "Projects").
const SESSIONS_PATTERN: &str = "refs/sessions/";

/// The three namespaces `GET /projects/{id}/branches` reports, in the order
/// [`list_branches`] sorts them into.
const BRANCH_PATTERNS: [&str; 3] = [HEADS_PATTERN, UPSTREAM_PATTERN, SESSIONS_PATTERN];

/// The argv of the one fetch every caller shares.
///
/// No refspec: the pair configured on `origin` is the contract
/// ([`FETCH_HEADS`], [`FETCH_TAGS`]), so a fetch cannot reach a namespace Mars
/// owns even by mistake.
const FETCH_ARGS: [&str; 5] = ["fetch", "--prune", "--quiet", "--end-of-options", "origin"];

/// How much life a credential must have left to be worth starting a fetch
/// with.
///
/// A PAT has no expiry the orchestrator can see, so this only matters to a
/// future minting provider (ADR 0002); five minutes is comfortably longer than
/// a fetch of a large repository and short enough that a token is not
/// re-minted for every one.
const CREDENTIAL_TTL: Duration = Duration::from_secs(300);

/// The upstream branch refspec: forced, because upstream tracking records what
/// upstream has, including a rewritten branch.
const FETCH_HEADS: &str = "+refs/heads/*:refs/remotes/origin/*";
/// The tag refspec. Tags land in `refs/tags/*` and are never seeded as heads.
const FETCH_TAGS: &str = "+refs/tags/*:refs/tags/*";

/// What `ls-remote --symref` prefixes the symbolic answer with.
const SYMREF_MARKER: &str = "ref: ";

/// What [`init_project_repo`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    /// The branch `HEAD` now points at: the caller's `requested_default`, or
    /// the one discovered from upstream's symbolic `HEAD`. The clone job
    /// writes it to `projects.default_branch`, which readiness requires
    /// (`docs/data-model.md`, `projects`).
    pub default_branch: String,
    /// The integration heads this call created, short names, in the order
    /// they were written. Empty on a re-run that found every head already
    /// seeded.
    pub seeded: Vec<String>,
}

/// Create and populate the project repository at
/// `DATA_DIR/projects/<project_id>/repo.git`.
///
/// The steps and their order are `ARCHITECTURE.md`, "Git model" (Project
/// clone); the module documentation above says what each one is for. The
/// project is the one the `guard` covers, so the lock and the repository
/// cannot disagree.
///
/// # Errors
///
/// - [`GitError::UnknownRef`] `HEAD` — upstream has no symbolic `HEAD` to
///   discover (a detached or empty upstream) and the caller named no branch.
/// - [`GitError::UnknownRef`] `<branch>` — the default branch, requested or
///   discovered, is not among the fetched upstream branches. The repository is
///   left on disk, so `retry-clone` reruns this routine unchanged once the
///   branch exists.
/// - [`GitError::Command`] — anything git refused, upstream being unreachable
///   or the credential being rejected among them. The caller turns it into
///   `status = error` with a human-readable `status_message`.
pub async fn init_project_repo(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    remote_url: &RemoteUrl,
    requested_default: Option<&str>,
    credential: Option<&GitCredential>,
) -> std::result::Result<InitOutcome, GitError> {
    let project_id = guard.project_id();
    let repo = paths.project_repo(project_id);

    // `git init --bare <path>` needs somewhere to create it; the clone job
    // creates this project's siblings (`claude/`, `shared/`) itself.
    std::fs::create_dir_all(paths.project_dir(project_id))?;

    GitCommand::new()
        .args(["init", "--bare", "--quiet", "--end-of-options"])
        .arg(&repo)
        .cwd(paths.project_dir(project_id))
        .run_ok()
        .await?;

    configure_origin(&repo, remote_url).await?;

    let default_branch = match requested_default {
        Some(name) => {
            // Through the ref parser so a name that could not be a branch is
            // refused before it reaches `symbolic-ref`, and so this agrees
            // with every other place a branch name is accepted.
            refs::GitRef::parse(&format!("{HEADS}{name}"))?;
            name.to_string()
        }
        None => discover_default_branch(&repo, paths, credential).await?,
    };

    fetch_origin(&repo, paths, credential).await?;

    let seeded = seed_integration_heads(&repo, &default_branch).await?;

    GitCommand::new()
        .args(["symbolic-ref", "--end-of-options", "HEAD"])
        .arg(format!("{HEADS}{default_branch}"))
        .cwd(&repo)
        .run_ok()
        .await?;

    info!(
        project_id = %project_id,
        git.default_branch = %default_branch,
        git.seeded = seeded.len(),
        "project repository initialised"
    );

    Ok(InitOutcome {
        default_branch,
        seeded,
    })
}

/// Refresh the upstream-tracking refs and tags of one project repository.
///
/// `git fetch --prune origin`, with the credential attached through a
/// temporary mode-0600 config when there is one. The refspecs come from the
/// repository's own `origin` configuration, so this updates
/// `refs/remotes/origin/*` and `refs/tags/*`, prunes the ones upstream no
/// longer has, and cannot touch `refs/heads/*`, `refs/sessions/*` or
/// `refs/handoffs/*` (`ARCHITECTURE.md`, "Git model", Ref ownership; ADR
/// 0017).
///
/// The caller holds the project git lock and passes the guard; the repository
/// is the one that guard covers. [`fetch_project`] is the composite operation
/// that takes the lock, finds the credential and records the fetch.
///
/// # Errors
///
/// [`GitError::Command`] for anything git refused — an unreachable upstream
/// and a rejected credential among them. Nothing was written in that case, so
/// the caller leaves `last_fetched_at` alone and retries: the cron job at its
/// next interval, the launcher after recording a `launch_warning`
/// (`ARCHITECTURE.md`, "Launch sequence").
pub async fn fetch_upstream(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    credential: Option<&GitCredential>,
) -> std::result::Result<(), GitError> {
    let project_id = guard.project_id();

    fetch_origin(&paths.project_repo(project_id), paths, credential).await?;

    debug!(project_id = %project_id, "project repository fetched");

    Ok(())
}

/// What [`fetch_project`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchOutcome {
    /// Whether a `git fetch` actually ran. `false` only when `max_age` was
    /// given and the recorded fetch was newer than that.
    pub fetched: bool,
    /// When the repository was last fetched: the timestamp this call wrote, or
    /// the one it found fresh enough to skip.
    pub at: DateTime<Utc>,
}

/// Fetch one project's upstream, under its git lock, and record it.
///
/// The one routine behind all three callers (`ARCHITECTURE.md`, "Git model",
/// Project clone): the cron mirror-fetch job passes [`GitActor::System`] and
/// no `max_age`, `POST /projects/{id}/fetch` passes the requesting user and no
/// `max_age`, and a fresh session launch passes the launching user and
/// `max_age = 30s`, which is what makes two launches in a row cost one fetch
/// (`ARCHITECTURE.md`, "Launch sequence").
///
/// The order is the documented one and the reason for it is ADR 0021: the
/// project row is read first, the git lock is taken next, the credential and
/// the fetch happen under it, and the lock is released before the short
/// transaction that writes `last_fetched_at`. No database transaction is ever
/// open while git runs, so a fetch of a large repository cannot hold a row
/// lock somebody else is waiting for.
///
/// Two concurrent fetches of one project serialise on the git lock. The second
/// re-runs the fetch rather than re-reading the row the first just wrote,
/// which is harmless: a fetch is idempotent, and the alternative — a second
/// read under the lock — would silently turn an explicit `POST .../fetch` into
/// a no-op whenever the cron job had just run.
///
/// # Errors
///
/// - [`Error::NotFound`] — no such project.
/// - [`Error::Conflict`] `project is not ready` — the project is still
///   `cloning` or in `error`, so there may be no repository to fetch and the
///   clone job owns it. Checked before the lock is taken, because a project
///   that is being cloned is holding it.
/// - [`Error::Git`] — the fetch failed; `last_fetched_at` is unchanged.
/// - [`Error::Database`], [`Error::Secrets`] — the row read, the credential
///   lookup or the recording failed.
pub async fn fetch_project(
    state: &AppState,
    project_id: Uuid,
    actor: &GitActor,
    max_age: Option<Duration>,
) -> Result<FetchOutcome> {
    let projects = ProjectRepository::new(&state.pool);
    let project = projects.find(project_id).await?.ok_or(Error::NotFound)?;

    if project.status != ProjectStatus::Ready {
        // Before the lock: the clone job holds it for the whole of a first
        // clone, and waiting minutes to answer 409 helps nobody.
        return Err(Error::Conflict("project is not ready".to_string()));
    }

    if let Some(at) = fresh_enough(&project, max_age) {
        debug!(project_id = %project_id, "the project repository was fetched recently enough");
        return Ok(FetchOutcome { fetched: false, at });
    }

    let paths = DataPaths::from_config(&state.config);

    {
        let guard = state.git_locks.lock(project_id).await;
        let credential = state
            .git_credentials
            .credential_for(project_id, actor, CREDENTIAL_TTL)
            .await?;

        fetch_upstream(&guard, &paths, credential.as_ref()).await?;
    }

    // Only after the command succeeded, and with the git lock already
    // released (ADR 0021).
    let mut tx = state.pool.begin().await?;
    let updated = projects
        .set_last_fetched_at(&mut tx, project_id)
        .await?
        .ok_or(Error::NotFound)?;
    tx.commit().await?;

    info!(project_id = %project_id, "project repository fetched from upstream");

    Ok(FetchOutcome {
        fetched: true,
        // The column is `NOT NULL` from the moment that statement ran; the
        // fallback is only so a schema surprise cannot panic here.
        at: updated.last_fetched_at.unwrap_or_else(Utc::now),
    })
}

/// The refs `GET /projects/{id}/branches` reports (`SPEC.md`, "Projects").
///
/// Integration heads, upstream-tracking refs and session refs, in that order
/// and alphabetically by API name within each. Tags, hand-off refs and
/// `refs/remotes/origin/HEAD` are not branches and are dropped by
/// [`refs::to_branch`].
///
/// Read-only, so it takes no guard: `for-each-ref` reports whatever the
/// repository says at the moment it runs, and a concurrent fetch can only move
/// upstream-tracking refs it has not read yet.
pub async fn list_branches(
    paths: &DataPaths,
    project_id: Uuid,
) -> std::result::Result<Vec<Branch>, GitError> {
    let repo = paths.project_repo(project_id);

    let mut branches: Vec<Branch> = refs::list(&repo, &BRANCH_PATTERNS)
        .await?
        .iter()
        .filter_map(refs::to_branch)
        .collect();

    // `for-each-ref` sorts by fully qualified name, which would interleave
    // nothing but would order `refs/heads/*` before `refs/remotes/*` before
    // `refs/sessions/*` by accident rather than by contract. Sort explicitly.
    branches.sort_by(|left, right| {
        kind_order(left.kind)
            .cmp(&kind_order(right.kind))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(branches)
}

/// The documented order of the three kinds: heads, upstream, sessions.
fn kind_order(kind: BranchKind) -> u8 {
    match kind {
        BranchKind::Head => 0,
        BranchKind::Upstream => 1,
        BranchKind::Session => 2,
    }
}

/// When `max_age` says the recorded fetch is recent enough to skip, the
/// timestamp it was recorded at.
///
/// A `last_fetched_at` in the future — a clock that moved backwards — counts
/// as fresh rather than as arbitrarily old: the alternative is fetching on
/// every launch until the clock catches up.
fn fresh_enough(project: &Project, max_age: Option<Duration>) -> Option<DateTime<Utc>> {
    let max_age = max_age?;
    let at = project.last_fetched_at?;

    match Utc::now().signed_duration_since(at).to_std() {
        Ok(age) => (age < max_age).then_some(at),
        // Negative, so the recorded fetch is ahead of this clock.
        Err(_) => Some(at),
    }
}

/// Delete `repo.git` entirely.
///
/// Project deletion, after every session of the project is gone
/// (`ARCHITECTURE.md`, "Storage"). Idempotent: a repository that is not there
/// is the wanted state, not a failure. The project directory itself stays;
/// removing the CLI state and shared directories beside it is the caller's,
/// which knows what else it created.
pub async fn remove_project_repo(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
) -> std::result::Result<(), GitError> {
    let repo = paths.project_repo(guard.project_id());

    match std::fs::remove_dir_all(&repo) {
        Ok(()) => {
            info!(project_id = %guard.project_id(), "project repository removed");
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(GitError::Io(err)),
    }
}

/// Point `origin` at `remote_url` and write the settings that make this
/// repository a Mars project repository rather than a mirror.
///
/// `--replace-all` throughout, so a re-run leaves exactly one value per key
/// and exactly the two fetch refspecs rather than appending a third.
async fn configure_origin(
    repo: &Path,
    remote_url: &RemoteUrl,
) -> std::result::Result<(), GitError> {
    // `git remote add` refuses a remote that exists, and `git remote remove`
    // would take the upstream-tracking refs with it, so an existing `origin`
    // has its URL replaced instead.
    let configured = GitCommand::new()
        .args(["config", "--get", "--end-of-options", "remote.origin.url"])
        .cwd(repo)
        .run()
        .await?;

    let remote_args: [&str; 3] = if configured.status == 0 {
        ["remote", "set-url", "origin"]
    } else {
        ["remote", "add", "origin"]
    };
    GitCommand::new()
        .args(remote_args)
        .arg(remote_url.as_str())
        .cwd(repo)
        .run_ok()
        .await?;

    // The first refspec replaces whatever was there, the second is added to
    // it: the pair is the contract, and `remote add` wrote the first one on
    // its own.
    set_config(repo, "--replace-all", "remote.origin.fetch", FETCH_HEADS).await?;
    set_config(repo, "--add", "remote.origin.fetch", FETCH_TAGS).await?;

    // Objects a session clone borrows through its alternates file must never
    // be collected (ADR 0001).
    set_config(repo, "--replace-all", "gc.auto", "0").await?;
    set_config(repo, "--replace-all", "gc.pruneExpire", "never").await?;

    // Not part of the documented contract: reflogs for `refs/heads/*` and
    // `refs/sessions/*`, which a bare repository otherwise does not keep.
    set_config(repo, "--replace-all", "core.logAllRefUpdates", "true").await?;

    Ok(())
}

/// One `git config <mode> <key> <value>`.
async fn set_config(
    repo: &Path,
    mode: &str,
    key: &str,
    value: &str,
) -> std::result::Result<(), GitError> {
    GitCommand::new()
        .args(["config", mode, "--end-of-options", key, value])
        .cwd(repo)
        .run_ok()
        .await?;

    Ok(())
}

/// Ask upstream which branch its `HEAD` points at.
///
/// `git ls-remote --symref origin HEAD`, the one network call the clone job
/// makes before the fetch. An upstream with a detached `HEAD`, or an empty
/// one, advertises no symbolic ref; there is nothing to discover then and the
/// caller has to supply `default_branch` itself.
async fn discover_default_branch(
    repo: &Path,
    paths: &DataPaths,
    credential: Option<&GitCredential>,
) -> std::result::Result<String, GitError> {
    let output = run_with_credential(
        GitCommand::new()
            .args([
                "ls-remote",
                "--symref",
                "--end-of-options",
                "origin",
                "HEAD",
            ])
            .cwd(repo),
        credential,
        paths,
    )
    .await?;

    parse_symref_head(&output.stdout).ok_or_else(|| GitError::UnknownRef("HEAD".to_string()))
}

/// The branch named by the `ref: refs/heads/<name>\tHEAD` line of
/// `ls-remote --symref`, if it is there.
///
/// The other lines are ordinary `<oid>\t<ref>` pairs, and a symref pointing
/// anywhere but `refs/heads/` is not a branch Mars can seed, so both are
/// ignored rather than guessed at.
fn parse_symref_head(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let rest = line.strip_prefix(SYMREF_MARKER)?;
        let (target, name) = rest.split_once('\t')?;
        if name.trim() != "HEAD" {
            return None;
        }

        let branch = target.strip_prefix(HEADS)?;
        (!branch.is_empty()).then(|| branch.to_string())
    })
}

/// Create one integration head per fetched upstream branch that does not have
/// one yet, and check that the default branch is among them.
///
/// Existing heads are never touched: `refs/heads/<b>` is Mars's and changes
/// only through an explicit integration operation (ADR 0017), so a re-run
/// after upstream advanced seeds nothing and moves nothing. Tags are not
/// listed here at all, so the second fetch refspec cannot produce a head.
async fn seed_integration_heads(
    repo: &Path,
    default_branch: &str,
) -> std::result::Result<Vec<String>, GitError> {
    let mut known: HashSet<String> = refs::list(repo, &[HEADS_PATTERN])
        .await?
        .into_iter()
        .filter_map(|entry| entry.full_name.strip_prefix(HEADS).map(str::to_string))
        .collect();

    let mut seeded = Vec::new();
    for entry in refs::list(repo, &[UPSTREAM_PATTERN]).await? {
        let Some(name) = entry.full_name.strip_prefix(UPSTREAM) else {
            continue;
        };
        if name == "HEAD" || known.contains(name) {
            continue;
        }

        // No `expected_old`: the head is known not to exist, and a directory
        // or file conflict with another branch name (`feature` against
        // `feature/x`) is git's own refusal to report, as `GitError::Command`.
        refs::update(repo, &format!("{HEADS}{name}"), &entry.commit, None).await?;
        known.insert(name.to_string());
        seeded.push(name.to_string());
    }

    if !known.contains(default_branch) {
        return Err(GitError::UnknownRef(default_branch.to_string()));
    }

    Ok(seeded)
}

/// The one `git fetch --prune origin` in the crate.
///
/// [`init_project_repo`]'s first fetch and [`fetch_upstream`]'s recurring one
/// are the same command by construction rather than by two argv lists that
/// have to be kept equal (`ARCHITECTURE.md`, "Git model", Project clone: "a
/// fresh session launch and `POST /projects/{id}/fetch` run the same fetch").
async fn fetch_origin(
    repo: &Path,
    paths: &DataPaths,
    credential: Option<&GitCredential>,
) -> std::result::Result<(), GitError> {
    run_with_credential(
        GitCommand::new().args(FETCH_ARGS).cwd(repo),
        credential,
        paths,
    )
    .await?;

    Ok(())
}

/// Run `command`, with `credential` attached through a temporary config when
/// there is one.
///
/// The config is deleted before this returns whichever way the command went.
/// A git failure is what the caller acts on, so it wins over a failed
/// deletion, which is logged instead — by which point the credential file is
/// the finding, not the error.
async fn run_with_credential(
    command: GitCommand,
    credential: Option<&GitCredential>,
    paths: &DataPaths,
) -> std::result::Result<GitOutput, GitError> {
    let Some(credential) = credential else {
        return command.run_ok().await;
    };

    let config = CredentialConfig::write(credential, &paths.tmp())?;
    let result = command.config_global(config.path()).run_ok().await;
    let removed = config.close();

    match result {
        Ok(output) => removed.map(|()| output),
        Err(err) => {
            if let Err(cleanup) = removed {
                warn!(error = %cleanup, "a temporary git credential config was left behind");
            }
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `git ls-remote --symref <url> HEAD` prints, with an obviously fake
    /// object id.
    const SYMREF_OUTPUT: &str =
        "ref: refs/heads/main\tHEAD\n0000000000000000000000000000000000000000\tHEAD\n";

    #[test]
    fn the_symref_line_names_the_default_branch() {
        assert_eq!(
            parse_symref_head(SYMREF_OUTPUT).as_deref(),
            Some("main"),
            "the first line of ls-remote --symref is the answer"
        );
    }

    #[test]
    fn a_branch_name_with_a_slash_survives_the_parse() {
        let output = "ref: refs/heads/release/2.0\tHEAD\n";

        assert_eq!(parse_symref_head(output).as_deref(), Some("release/2.0"));
    }

    #[test]
    fn output_without_a_symref_line_names_no_branch() {
        // A detached upstream HEAD: the oid is advertised, the symref is not.
        let detached = "0000000000000000000000000000000000000000\tHEAD\n";

        assert_eq!(parse_symref_head(detached), None);
        assert_eq!(
            parse_symref_head(""),
            None,
            "an empty upstream says nothing"
        );
    }

    #[test]
    fn a_symref_outside_the_head_namespace_is_not_a_branch() {
        for output in [
            "ref: refs/remotes/upstream/main\tHEAD\n",
            "ref: refs/heads/\tHEAD\n",
            "ref: refs/heads/main\trefs/heads/other\n",
        ] {
            assert_eq!(parse_symref_head(output), None, "accepted {output:?}");
        }
    }
}
