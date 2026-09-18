//! [`push`], the only command in the crate that writes to upstream.
//!
//! `ARCHITECTURE.md`, "Git model" (Merge, rebase, push) is the contract, and
//! it is a narrow one: a push sends *exactly* the selected integration head or
//! session ref to `refs/heads/<remote_branch>` with an explicit refspec, never
//! a mirror push. There is no `--mirror`, no `--all` and no `--tags` here, and
//! there is no code path that pushes more than one ref, so no operation can
//! ever publish a session's work, a hand-off ref or a tag as a side effect of
//! publishing something else (ADR 0007, ADR 0017).
//!
//! **A rejection is a state, not a fault.** When upstream has advanced
//! incompatibly git refuses the update and changes nothing, anywhere: not
//! upstream, and not the integration head or session ref in the project
//! repository. That is [`GitError::NonFastForward`], which is 409 at
//! `POST /projects/{pid}/git/push` and `conflict` on the MCP `push` tool
//! (`SPEC.md`, "Git"; "MCP tool contracts"). The caller fetches, integrates
//! the upstream changes and retries, and a local merge that was already made
//! is still there — a failed push never rolls one back. Every other refusal —
//! a protected branch, a declined hook, an unreachable remote — is
//! [`GitError::Command`]: a 500 with a generic message, the detail in the log.
//!
//! **Why `--porcelain`.** Git reports a per-ref outcome on stdout in a
//! machine-readable form and exits non-zero for *any* refusal, so the exit
//! code alone cannot tell a non-fast-forward from a declined hook. The
//! porcelain line can, and it is a documented format rather than the
//! human-readable prose on stderr, which is localised and changes between
//! versions (ADR 0011: parsing is limited to porcelain formats).
//!
//! **What this does not write.** After a successful push git updates
//! `refs/remotes/origin/<remote_branch>` itself, because the fetch refspec
//! configured on `origin` (`+refs/heads/*:refs/remotes/origin/*`,
//! [`mirror`](super::mirror)) matches the ref that moved. Nothing here calls
//! [`refs::update`](super::refs::update): upstream tracking is git's own
//! bookkeeping and writing it by hand could only ever disagree with it.
//!
//! **Credentials.** The push is the one command here that talks to the remote,
//! so it is the one that gets a credential: an `http.extraHeader` written into
//! a temporary mode-0600 config selected through `GIT_CONFIG_GLOBAL` and
//! deleted before this function returns, on the failure path as well (ADR
//! 0002; `ARCHITECTURE.md`, "Git model", Credentials). It is never an
//! argument, so neither the argv [`GitError::Command`] carries nor the stderr
//! it quotes can contain one (`CLAUDE.md` rule 3). The `secret_uses` row is
//! written by the provider when the service obtains the credential, under
//! `GitActor::User` for REST and `GitActor::Session` for MCP.
//!
//! **Locking.** The caller holds the project git lock and passes the guard;
//! nothing here acquires one (`ARCHITECTURE.md`, "Git model",
//! Serialization).

use super::mirror::run_with_credential_raw;
use super::refs::{GitRef, ResolvedRef};
use super::{DataPaths, GitCommand, GitCredential, GitError, ProjectGitGuard, session_branch};
use crate::prelude::*;

/// The upstream branch namespace a push writes into. Always `refs/heads/`: a
/// push publishes a branch, whatever namespace the local ref came from
/// (ADR 0007: a session's `refs/sessions/<id>` becomes
/// `refs/heads/session/<id>` upstream).
const HEADS: &str = "refs/heads/";

/// The only remote a project has, configured by
/// [`init_project_repo`](super::init_project_repo).
const ORIGIN: &str = "origin";

/// The prefix of a GitHub repository URL Mars can build a compare page for.
/// Only `https://`, and only this host: a project's stored `remote_url` is
/// `https://` by construction ([`RemoteUrl::parse`](crate::models::RemoteUrl)).
const GITHUB_HTTPS: &str = "https://github.com/";

/// The `.git` suffix a clone URL usually carries and a web URL never does.
const GIT_SUFFIX: &str = ".git";

/// The porcelain flag on the line of a ref git refused.
const REJECTED_FLAG: &str = "!";

/// The summary marker git writes for a rejection it decided itself, as opposed
/// to `[remote rejected]`, which the receiving end decided. The two are
/// different failures and the substring is deliberately exact: `[remote
/// rejected]` does not contain `[rejected]`.
const REJECTED: &str = "[rejected]";

/// The three reasons behind a `[rejected]` that mean "upstream moved": the
/// caller can fetch, integrate and retry, which is what
/// [`GitError::NonFastForward`] tells them to do.
///
/// `(stale info)` belongs here even though [`push`] never uses
/// `--force-with-lease` — it is the lease-specific spelling of the same
/// answer, and a future caller that gains a lease should not have to
/// rediscover it.
const FETCHABLE_REJECTIONS: [&str; 3] = ["(non-fast-forward)", "(fetch first)", "(stale info)"];

/// What a successful push published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOutcome {
    /// The upstream branch that now points at [`PushOutcome::commit`], as a
    /// short name: `main`, `session/<id>`, `feature/x`. This is what the REST
    /// body and the MCP tool return (`SPEC.md`, "Git").
    pub remote_branch: String,
    /// The commit that was published: the one the source ref was resolved to
    /// before the push, so the answer describes exactly what was sent rather
    /// than whatever the ref says afterwards.
    pub commit: String,
    /// The GitHub compare page for opening a pull request, when the project's
    /// remote is a GitHub one and the caller asked for it.
    ///
    /// Not part of the REST body, which stays `{remote_branch, commit}`
    /// (`SPEC.md`, "Git"): it travels in the `git` outcome event's `detail`,
    /// which is where the UI reads it from (`SPEC.md`, "User-facing features",
    /// Git operations).
    pub compare_url: Option<String>,
}

/// What [`push`] needs to build a compare link, when the caller wants one.
///
/// A pair rather than two more parameters: they are only ever supplied
/// together, and either one alone builds nothing.
#[derive(Debug, Clone, Copy)]
pub struct ComparePage<'a> {
    /// The project's stored `remote_url`. Never carries a credential
    /// ([`RemoteUrl::parse`](crate::models::RemoteUrl) rejects one).
    pub remote_url: &'a str,
    /// The project's default branch, which is the base side of the comparison.
    pub default_branch: &'a str,
}

/// Send one ref to `refs/heads/<remote_branch>` upstream.
///
/// `r` is the source, already resolved under the same project git lock, so the
/// commit reported back is the one that was sent. Its kind must be a push
/// source — an integration head or a session ref
/// ([`GitRef::is_push_source`]) — because those are the only two namespaces
/// Mars may publish (ADR 0017).
///
/// `remote_branch` defaults to the source's own name: `<name>` for a
/// [`GitRef::Head`] and `session/<sid>` for a [`GitRef::Session`], which is the
/// spelling `SPEC.md`, "MCP tool contracts" gives the `push` tool. A supplied
/// name is validated as a branch name by the same rules
/// [`GitRef::parse`] applies to a head, and a fully qualified `refs/…`
/// spelling is refused: the target namespace is not the caller's to choose.
/// Deleting an upstream branch is not a push — there is no empty source — so
/// an empty name is refused here rather than turning into one.
///
/// `force` adds `--force`, and only when the caller explicitly asked for it
/// (`SPEC.md`, "Git": REST force-pushes require `force: true`, MCP
/// additionally the profile's `push` permission). Deliberately not
/// `--force-with-lease`: a lease needs the caller to name the commit it
/// expects upstream to be at, and neither the endpoint nor the tool exposes
/// such a field, so the lease would have to be invented here — which is the
/// same as no lease, with a more reassuring name.
///
/// # Errors
///
/// - [`GitError::InvalidRef`] — `r` is not a push source, or `remote_branch`
///   is not a usable branch name (400).
/// - [`GitError::NonFastForward`] — upstream has moved on; nothing was
///   changed, locally or upstream (409).
/// - [`GitError::Command`] — anything else git refused or could not do: a
///   protected branch, a declined hook, an unreachable remote (500, generic).
pub async fn push(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    r: &ResolvedRef,
    remote_branch: Option<&str>,
    force: bool,
    credential: Option<&GitCredential>,
    compare: Option<ComparePage<'_>>,
) -> std::result::Result<PushOutcome, GitError> {
    if !r.git_ref.is_push_source() {
        return Err(GitError::InvalidRef(r.git_ref.api_name()));
    }
    // Unreachable for the two push-source kinds, both of which have a full
    // name; an `ok_or_else` rather than an `expect` because a panic is not a
    // failure mode this module is allowed to have.
    let source = r
        .git_ref
        .full_name()
        .ok_or_else(|| GitError::InvalidRef(r.git_ref.api_name()))?;

    let remote_branch = match remote_branch {
        Some(requested) => validate_remote_branch(requested)?,
        None => default_remote_branch(&r.git_ref)?,
    };

    let repo = paths.project_repo(guard.project_id());
    let argv = push_argv(force, &format!("{source}:{HEADS}{remote_branch}"));

    let output =
        run_with_credential_raw(GitCommand::new().args(&argv).cwd(&repo), credential, paths)
            .await?;

    match rejection(&output.stdout) {
        Some(Rejection::NonFastForward) => {
            debug!(
                project_id = %guard.project_id(),
                git.remote_branch = %remote_branch,
                "a push was rejected as non-fast-forward; every local ref is unchanged"
            );
            return Err(GitError::NonFastForward { remote_branch });
        }
        // A refusal the caller cannot resolve by fetching, and one git may
        // report while still exiting 0 has never been observed — but the exit
        // code is not what decides this, the porcelain line is.
        Some(Rejection::Other) => return Err(command_error(argv, &output)),
        None if output.status != 0 => return Err(command_error(argv, &output)),
        None => {}
    }

    let compare_url = compare
        .and_then(|page| github_compare_url(page.remote_url, page.default_branch, &remote_branch));

    debug!(
        project_id = %guard.project_id(),
        git.source = %r.git_ref.api_name(),
        git.remote_branch = %remote_branch,
        commit = %r.commit,
        forced = force,
        "pushed one ref upstream"
    );

    Ok(PushOutcome {
        remote_branch,
        commit: r.commit.clone(),
        compare_url,
    })
}

/// The GitHub compare page for opening a pull request from `head` against
/// `base`, or `None` when the remote is not a GitHub one.
///
/// `https://github.com/<owner>/<repo>` with or without a `.git` suffix and
/// with or without a trailing slash; anything else — another host, a deeper
/// path, a missing repository name — produces nothing rather than a link that
/// would 404. The branch names go in as they are: a git branch name cannot
/// contain a space, a `?`, a `~`, a `^`, a `:` or a `\`
/// ([`GitRef::parse`]'s rules), so nothing that reaches here can change the
/// shape of the URL.
pub fn github_compare_url(remote_url: &str, base: &str, head: &str) -> Option<String> {
    let path = remote_url.trim().strip_prefix(GITHUB_HTTPS)?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(GIT_SUFFIX).unwrap_or(path);
    let path = path.trim_end_matches('/');

    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }

    Some(format!(
        "{GITHUB_HTTPS}{owner}/{repo}/compare/{base}...{head}?expand=1"
    ))
}

/// The argv of the one push in the crate, as owned strings so the failure can
/// carry exactly what was run.
///
/// `--end-of-options` after the flags, so neither the remote name nor the
/// refspec could be read as an option even if validation had let one through.
fn push_argv(force: bool, refspec: &str) -> Vec<String> {
    let mut argv = vec!["push".to_string(), "--porcelain".to_string()];
    if force {
        argv.push("--force".to_string());
    }
    argv.push("--end-of-options".to_string());
    argv.push(ORIGIN.to_string());
    argv.push(refspec.to_string());
    argv
}

/// The upstream branch a source ref publishes as when the caller named none.
///
/// Visible to the rest of `git/` so the service can name the branch a failed
/// push was aiming at in its `git` outcome event without restating the rule.
pub(super) fn default_remote_branch(git_ref: &GitRef) -> std::result::Result<String, GitError> {
    match git_ref {
        GitRef::Head(name) => Ok(name.clone()),
        // `session/<sid>`: the same branch name the session's own clone uses,
        // which is what makes the upstream branch recognisable as that
        // session's work (ADR 0007).
        GitRef::Session(id) => Ok(session_branch(*id)),
        other => Err(GitError::InvalidRef(other.api_name())),
    }
}

/// Accept `requested` as a short upstream branch name.
///
/// Through [`GitRef::parse`] rather than a second set of rules, so a name that
/// is a legal integration head is exactly a name that is a legal push target.
/// The `refs/` and empty cases are checked first only so the error names what
/// the caller actually sent instead of the qualified form built from it.
///
/// Visible to the rest of `git/` so the service can refuse a malformed name
/// before it takes the project git lock and syncs anything.
pub(super) fn validate_remote_branch(requested: &str) -> std::result::Result<String, GitError> {
    let invalid = || GitError::InvalidRef(requested.to_string());

    if requested.is_empty() || requested.starts_with("refs/") {
        return Err(invalid());
    }

    match GitRef::parse(&format!("{HEADS}{requested}")).map_err(|_| invalid())? {
        GitRef::Head(branch) => Ok(branch),
        _ => Err(invalid()),
    }
}

/// Why git refused a ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rejection {
    /// Upstream has advanced incompatibly. A state the caller resolves by
    /// fetching and integrating, and the one rejection that is not a fault.
    NonFastForward,
    /// Anything else git or the receiving end refused: a protected branch, a
    /// declined hook, a remote failure.
    Other,
}

/// Classify the first ref line `git push --porcelain` marked as refused.
///
/// The format is `<flag>\t<from>:<to>\t<summary>`, one line per ref, between a
/// `To <url>` line and a final `Done`. Only the flag and the summary markers
/// are matched — never the prose on stderr, which is localised. Exactly one
/// refspec is ever pushed, so there is at most one such line.
fn rejection(stdout: &str) -> Option<Rejection> {
    stdout.lines().find_map(|line| {
        let mut fields = line.split('\t');
        if fields.next() != Some(REJECTED_FLAG) {
            return None;
        }

        // `<from>:<to>` is skipped; the summary is the third field.
        let summary = fields.nth(1).unwrap_or_default();
        let fetchable = summary.contains(REJECTED)
            && FETCHABLE_REJECTIONS
                .iter()
                .any(|reason| summary.contains(reason));

        Some(if fetchable {
            Rejection::NonFastForward
        } else {
            Rejection::Other
        })
    })
}

/// The internal fault a refusal the caller cannot act on becomes.
///
/// A negative status is [`GitCommand::run`]'s stand-in for "the child never
/// exited", which [`GitError::Command`] spells as `code: None`.
fn command_error(args: Vec<String>, output: &super::GitOutput) -> GitError {
    GitError::Command {
        args,
        code: (output.status >= 0).then_some(output.status),
        stderr: output.stderr.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed session id, so the expectations read at a glance.
    const SESSION: &str = "11111111-2222-3333-4444-555555555555";

    fn session_id() -> uuid::Uuid {
        uuid::Uuid::parse_str(SESSION).expect("a fixed uuid")
    }

    #[test]
    fn a_fast_forward_push_is_not_a_rejection() {
        let stdout = "To https://github.com/acme/widgets.git\n \trefs/heads/main:refs/heads/main\t1111111..2222222\nDone\n";

        assert_eq!(rejection(stdout), None);
    }

    #[test]
    fn a_new_branch_and_an_up_to_date_push_are_not_rejections() {
        for stdout in [
            "To https://example.invalid/r.git\n*\trefs/sessions/x:refs/heads/session/x\t[new branch]\nDone\n",
            "To https://example.invalid/r.git\n=\trefs/heads/main:refs/heads/main\t[up to date]\nDone\n",
            "To https://example.invalid/r.git\n+\trefs/heads/main:refs/heads/main\t1111111...2222222 (forced update)\nDone\n",
        ] {
            assert_eq!(rejection(stdout), None, "{stdout}");
        }
    }

    #[test]
    fn each_documented_rejection_reason_is_a_non_fast_forward() {
        for reason in FETCHABLE_REJECTIONS {
            let stdout = format!(
                "To https://example.invalid/r.git\n!\trefs/heads/main:refs/heads/main\t[rejected] {reason}\nDone\n"
            );

            assert_eq!(
                rejection(&stdout),
                Some(Rejection::NonFastForward),
                "{stdout}"
            );
        }
    }

    #[test]
    fn a_protected_branch_is_a_fault_and_not_a_non_fast_forward() {
        // `[remote rejected]` is the receiving end's refusal, and it does not
        // contain the `[rejected]` marker the branch above matches on.
        let stdout = "To https://github.com/acme/widgets.git\n!\trefs/heads/main:refs/heads/main\t[remote rejected] (protected branch hook declined)\nDone\n";

        assert_eq!(rejection(stdout), Some(Rejection::Other));
    }

    #[test]
    fn a_rejection_without_a_known_reason_is_a_fault() {
        for stdout in [
            "!\trefs/heads/main:refs/heads/main\t[rejected] (already exists)\n",
            "!\trefs/heads/main:refs/heads/main\t[no match]\n",
            "!\trefs/heads/main:refs/heads/main\t[remote failure]\n",
        ] {
            assert_eq!(rejection(stdout), Some(Rejection::Other), "{stdout}");
        }
    }

    #[test]
    fn the_reason_is_only_read_from_the_summary_field() {
        // A branch literally named after a rejection reason must not make a
        // successful push look like one, and the `To` line is never a ref
        // line whatever it says.
        let stdout = "To https://example.invalid/(non-fast-forward).git\n \trefs/heads/x:refs/heads/[rejected] (fetch first)\t111..222\nDone\n";

        assert_eq!(rejection(stdout), None);
    }

    #[test]
    fn the_argv_is_the_documented_command_and_nothing_else() {
        let plain = push_argv(false, "refs/heads/main:refs/heads/main");

        assert_eq!(
            plain,
            vec![
                "push",
                "--porcelain",
                "--end-of-options",
                "origin",
                "refs/heads/main:refs/heads/main",
            ]
        );
        assert!(
            !plain
                .iter()
                .any(|arg| ["--mirror", "--all", "--tags"].contains(&arg.as_str())),
            "{plain:?}"
        );
    }

    #[test]
    fn force_is_the_only_difference_a_forced_push_makes() {
        let forced = push_argv(true, "refs/heads/main:refs/heads/main");

        assert_eq!(
            forced,
            vec![
                "push",
                "--porcelain",
                "--force",
                "--end-of-options",
                "origin",
                "refs/heads/main:refs/heads/main",
            ]
        );
        // Explicitly not a lease: the API exposes no expected remote commit.
        assert!(
            !forced
                .iter()
                .any(|arg| arg.starts_with("--force-with-lease")),
            "{forced:?}"
        );
    }

    #[test]
    fn a_source_publishes_under_its_own_name_by_default() {
        assert_eq!(
            default_remote_branch(&GitRef::Head("feature/x".to_string()))
                .expect("an integration head is a push source"),
            "feature/x"
        );
        assert_eq!(
            default_remote_branch(&GitRef::Session(session_id()))
                .expect("a session ref is a push source"),
            format!("session/{SESSION}")
        );
    }

    #[test]
    fn nothing_but_a_head_or_a_session_has_a_default_remote_branch() {
        for git_ref in [
            GitRef::Upstream("main".to_string()),
            GitRef::Tag("v1.0.0".to_string()),
            GitRef::Handoff(session_id()),
            GitRef::Commit("a".repeat(40)),
        ] {
            assert!(
                matches!(
                    default_remote_branch(&git_ref),
                    Err(GitError::InvalidRef(_))
                ),
                "{git_ref:?}"
            );
        }
    }

    #[test]
    fn a_requested_remote_branch_is_a_branch_name() {
        for name in ["main", "feature/x", "release-2"] {
            assert_eq!(
                validate_remote_branch(name).expect("a usable branch name"),
                name
            );
        }
    }

    #[test]
    fn a_qualified_or_empty_remote_branch_is_refused() {
        for name in [
            "",
            "refs/heads/x",
            "refs/sessions/x",
            "-force",
            "a..b",
            "with space",
            "trailing/",
            "x.lock",
        ] {
            let error = validate_remote_branch(name)
                .expect_err("a name that is not a usable branch is refused");

            assert!(
                matches!(&error, GitError::InvalidRef(named) if named == name),
                "{name:?} produced {error:?}"
            );
        }
    }

    #[test]
    fn a_github_remote_gets_a_compare_page_with_or_without_the_git_suffix() {
        let expected = "https://github.com/acme/widgets/compare/main...feature/x?expand=1";

        for url in [
            "https://github.com/acme/widgets.git",
            "https://github.com/acme/widgets",
            "https://github.com/acme/widgets/",
            "https://github.com/acme/widgets.git/",
            "  https://github.com/acme/widgets.git  ",
        ] {
            assert_eq!(
                github_compare_url(url, "main", "feature/x").as_deref(),
                Some(expected),
                "{url}"
            );
        }
    }

    #[test]
    fn a_session_branch_compares_against_the_default_branch() {
        assert_eq!(
            github_compare_url(
                "https://github.com/acme/widgets.git",
                "main",
                &format!("session/{SESSION}")
            )
            .as_deref(),
            Some(
                format!(
                    "https://github.com/acme/widgets/compare/main...session/{SESSION}?expand=1"
                )
                .as_str()
            )
        );
    }

    #[test]
    fn anything_that_is_not_a_github_repository_url_gets_no_compare_page() {
        for url in [
            "https://gitlab.example.invalid/acme/widgets.git",
            "https://git.example.invalid/acme/widgets",
            "http://github.com/acme/widgets.git",
            "https://www.github.com/acme/widgets.git",
            "https://github.com/acme",
            "https://github.com/acme/",
            "https://github.com/",
            "https://github.com/acme/widgets/extra",
            "/data/tmp/upstream.git",
        ] {
            assert_eq!(github_compare_url(url, "main", "x"), None, "{url}");
        }
    }
}
