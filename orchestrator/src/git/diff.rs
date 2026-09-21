//! The two read-only queries: the diff between two fixed commits and the list
//! of session branches with their ahead/behind counts.
//!
//! `ARCHITECTURE.md`, "Git model" (Diff): both run `git` directly against the
//! project repository. Neither makes a temporary clone, neither writes a ref
//! and neither emits an event, so the session view's "Changes" panel and the
//! task's revision view can ask as often as they like (`SPEC.md`, "Frontend":
//! the panel refetches on every `git` event).
//!
//! **Fixed commits.** [`diff`] takes two [`ResolvedRef`]s rather than two
//! names. Ref resolutions for a read-only diff are captured under the project
//! git lock and the git commands then run outside it
//! (`ARCHITECTURE.md`, "Git model", Serialization), so nothing here takes a
//! lock or accepts a guard. For the same reason nothing here syncs: when the
//! head is a session, the service task calls
//! [`session::fetch_back`](super::session::fetch_back) — which is silent by
//! design, so the panel's refresh rule cannot trigger itself — and resolves
//! the ref afterwards.
//!
//! **Merge base, not `base`.** The patch runs from `merge-base(base, head)` to
//! `head`. A session branched from `main` at commit X still shows only its own
//! work after `main` has moved to Y; comparing against `main` itself would
//! report everybody else's commits as deletions.
//!
//! **Renames off.** The three `git diff` invocations pass `--no-renames`,
//! which is the meaning of "without `-M`": rename detection has been on by
//! default for porcelain `git diff` since git 2.9, and with it on
//! `--name-status` and `--numstat` describe different sets of paths — one
//! `R` record against two `numstat` lines. A rename therefore appears as a
//! `D` and an `A`, which is what the frontend wants anyway: it lists files
//! with their counts, not rename arrows.
//!
//! **Literal paths.** The two file-list runs pass `-z`, which prints every
//! path verbatim. The patch cannot take `-z`, and by default git C-quotes a
//! path with any byte outside printable ASCII in it, so the patch run passes
//! `-c core.quotePath=false`: the `diff --git` header of `föo.png` then names
//! the same path the `files` entry does, which is what lets the frontend match
//! a patch block to a file row.
//!
//! **No repository-configured helpers.** `--no-ext-diff` and `--no-textconv`
//! are on every invocation. The project repository is orchestrator-owned and
//! is never given a diff driver, but a diff of agent-authored content must not
//! be able to run a program named by anything inside it (ADR 0011, ADR 0019).

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;

use uuid::Uuid;

use super::refs::{GitRef, ResolvedRef};
use super::{DataPaths, GitCommand, GitError, refs};
use crate::models::{Diff, DiffFile, DiffStatus, SessionBranch, is_commit_id};
use crate::prelude::*;

/// The patch size above which a [`Diff`] is truncated (`SPEC.md`, "Git";
/// `ARCHITECTURE.md`, "Git model", Diff).
///
/// A byte limit rather than a line limit, because it is a browser's memory it
/// protects. [`diff`] takes it as an argument so a caller can ask for less;
/// the endpoint passes this.
pub const MAX_PATCH_BYTES: usize = 1024 * 1024;

/// The message [`GitError::UnknownRef`] carries when two commits share no
/// ancestor. A 400 at the route, which is what an unrelated-histories pair is:
/// something the caller asked for that cannot be answered.
const NO_MERGE_BASE: &str = "no merge base";

/// The integration-head namespace, for qualifying a bare `default_branch`.
const HEADS: &str = "refs/heads/";

/// The session-work namespace. A prefix, not a glob: `for-each-ref` patterns
/// match whole path components, so `refs/sessions/*` would be the same thing
/// written less clearly and `*` would not cross a `/`.
const SESSIONS: &str = "refs/sessions/";

/// The flags every `git diff` here carries, in front of the mode-selecting
/// one. See the module documentation for why each is there.
const DIFF_FLAGS: [&str; 4] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-renames",
];

/// The diff between two commits in the project repository.
///
/// The files come from `--name-status` and `--numstat` — one run each, joined
/// by path — and the patch from an ordinary `git diff`, all three from
/// `merge-base(base, head)` to `head`. `patch` is cut at the last character
/// boundary at or before `limit` when it is longer than that, and `truncated`
/// says so; a patch exactly `limit` bytes long is not truncated.
///
/// `base` and `head` are already-resolved refs: their `api_name()` is what the
/// response carries, and their commits are what git is given, so a ref that
/// moves between the resolution and this call cannot change the answer.
///
/// # Errors
///
/// - [`GitError::UnknownRef`] `"no merge base"` — the two commits have no
///   common ancestor. 400 at the route.
/// - [`GitError::Command`] — anything else git refused; an internal fault.
pub async fn diff(
    paths: &DataPaths,
    project_id: Uuid,
    base: &ResolvedRef,
    head: &ResolvedRef,
    limit: usize,
) -> std::result::Result<Diff, GitError> {
    let repo = paths.project_repo(project_id);
    let merge_base = merge_base(&repo, &base.commit, &head.commit).await?;

    let name_status = run_diff(&repo, &["--name-status", "-z"], &merge_base, head).await?;
    let numstat = run_diff(&repo, &["--numstat", "-z"], &merge_base, head).await?;
    let patch = run_patch(&repo, &merge_base, head).await?;

    let (patch, truncated) = truncate_patch(patch, limit);

    debug!(
        project_id = %project_id,
        git.base = %base.git_ref.api_name(),
        git.head = %head.git_ref.api_name(),
        commit = %merge_base,
        "computed a diff from the merge base"
    );

    Ok(Diff {
        base: base.git_ref.api_name(),
        head: head.git_ref.api_name(),
        merge_base,
        files: build_files(&name_status, &numstat),
        patch,
        truncated,
    })
}

/// Every `refs/sessions/*` in the project repository, with how far each one is
/// ahead of and behind the integration head named by `default_branch`.
///
/// Newest tip first: `updated_at` is the committer date of the branch's tip,
/// which is what the session-branch list and the `list_session_branches` MCP
/// tool are ordered by. Refs whose name is not a session id are skipped with a
/// `warn!` rather than failing the listing, exactly as [`refs::list`] treats a
/// line it cannot read.
///
/// # Errors
///
/// [`GitError::UnknownRef`] naming `default_branch` when `refs/heads/<it>` is
/// not there. A `ready` project always has it — `mirror::init_project_repo`
/// seeds the integration heads — so this is a repository that was never
/// initialised or was damaged.
pub async fn session_branches(
    paths: &DataPaths,
    project_id: Uuid,
    default_branch: &str,
) -> std::result::Result<Vec<SessionBranch>, GitError> {
    let repo = paths.project_repo(project_id);

    // Qualified before parsing, so a stored `default_branch` that is not a
    // usable branch name is refused here instead of reaching argv — the same
    // treatment `session::resolve_base` gives it.
    let full_base = format!("{HEADS}{default_branch}");
    let base = GitRef::parse(&full_base)?;
    let resolved = refs::resolve(&repo, &base).await?;
    if resolved.git_ref != base {
        // `refs::resolve` retries a missing head as a tag of the same name.
        // Ahead/behind is measured against the integration head and nothing
        // else, so a tag standing in for it is a missing head.
        return Err(GitError::UnknownRef(default_branch.to_string()));
    }

    let entries = refs::list(&repo, &[SESSIONS]).await?;
    let mut branches = Vec::with_capacity(entries.len());

    for entry in entries {
        let Ok(GitRef::Session(session_id)) = GitRef::parse(&entry.full_name) else {
            warn!(
                git.refname = %entry.full_name,
                "skipping a session ref whose name is not a session id"
            );
            continue;
        };

        let (ahead, behind) = ahead_behind(&repo, &full_base, &entry.full_name).await?;

        branches.push(SessionBranch {
            session_id,
            git_ref: entry.full_name,
            commit: entry.commit,
            ahead,
            behind,
            base: default_branch.to_string(),
            updated_at: entry.committer_date,
        });
    }

    // A stable sort, so refs whose tips share a committer date — git records
    // whole seconds — keep `for-each-ref`'s order rather than an arbitrary one.
    branches.sort_by_key(|branch| Reverse(branch.updated_at));

    Ok(branches)
}

/// `git merge-base <base> <head>`.
///
/// Exit 1 with nothing on standard output is git's way of saying the two
/// commits share no ancestor, which is an answer rather than a failure, so
/// this goes through [`GitCommand::run`].
async fn merge_base(repo: &Path, base: &str, head: &str) -> std::result::Result<String, GitError> {
    let output = GitCommand::new()
        .args(["merge-base", "--end-of-options", base, head])
        .cwd(repo)
        .run()
        .await?;

    let failed = |code: Option<i32>, stderr: String| GitError::Command {
        args: vec![
            "merge-base".to_string(),
            "--end-of-options".to_string(),
            base.to_string(),
            head.to_string(),
        ],
        code,
        stderr,
    };

    if output.status != 0 {
        if output.status == 1 && output.stdout.trim().is_empty() {
            return Err(GitError::UnknownRef(NO_MERGE_BASE.to_string()));
        }
        return Err(failed(Some(output.status), output.stderr));
    }

    let oid = output.stdout.trim();
    if !is_commit_id(oid) {
        // git printed something that is not an object id after exiting 0,
        // which is not a state a caller can act on.
        return Err(failed(
            Some(0),
            "merge-base exited 0 without printing a full object id".to_string(),
        ));
    }

    Ok(oid.to_string())
}

/// One `git diff <shared flags> <mode flags> --end-of-options <merge base>
/// <head commit>`, as its standard output.
///
/// The output is already lossily decoded — [`GitCommand`] reads git's bytes as
/// UTF-8 with replacements, because a patch carries file content in whatever
/// encoding the repository uses — so everything downstream is a `String` that
/// can be truncated at a character boundary.
async fn run_diff(
    repo: &Path,
    mode: &[&str],
    merge_base: &str,
    head: &ResolvedRef,
) -> std::result::Result<String, GitError> {
    Ok(GitCommand::new()
        .arg("diff")
        .args(DIFF_FLAGS)
        .args(mode)
        .args(["--end-of-options", merge_base, &head.commit])
        .cwd(repo)
        .run_ok()
        .await?
        .stdout)
}

/// The patch itself: [`run_diff`] with no mode flags and `core.quotePath` off.
///
/// The file list comes from `-z` runs, which are never quoted, but a patch
/// cannot be asked for `-z`: its paths are quoted whenever git thinks the
/// terminal needs it, which by default is any byte outside printable ASCII.
/// A `diff --git "a/f\303\266o.png" "b/f\303\266o.png"` header names a path
/// that no entry of `files` carries, so the panel could not match the block to
/// the row. Off here rather than in the repository's config: a config value is
/// something a fetched repository could carry, and every invocation in this
/// module states what it needs (`ARCHITECTURE.md`, "Git model", Diff).
async fn run_patch(
    repo: &Path,
    merge_base: &str,
    head: &ResolvedRef,
) -> std::result::Result<String, GitError> {
    Ok(GitCommand::new()
        .args(["-c", "core.quotePath=false"])
        .arg("diff")
        .args(DIFF_FLAGS)
        .args(["--end-of-options", merge_base, &head.commit])
        .cwd(repo)
        .run_ok()
        .await?
        .stdout)
}

/// `git rev-list --left-right --count <base>...<head>` as `(ahead, behind)`.
///
/// The left count is what `base` has and `head` does not, which is `behind`;
/// the right count is `ahead`.
async fn ahead_behind(
    repo: &Path,
    base: &str,
    head: &str,
) -> std::result::Result<(u32, u32), GitError> {
    let range = format!("{base}...{head}");
    let output = GitCommand::new()
        .args(["rev-list", "--left-right", "--count", "--end-of-options"])
        .arg(&range)
        .cwd(repo)
        .run_ok()
        .await?;

    let mut counts = output.stdout.split_whitespace();
    match (
        counts.next().map(str::parse::<u32>),
        counts.next().map(str::parse::<u32>),
    ) {
        (Some(Ok(behind)), Some(Ok(ahead))) => Ok((ahead, behind)),
        _ => Err(GitError::Command {
            args: vec![
                "rev-list".to_string(),
                "--left-right".to_string(),
                "--count".to_string(),
                range,
            ],
            code: Some(0),
            stderr: "rev-list --count printed something other than two numbers".to_string(),
        }),
    }
}

/// Join the two `-z` listings into the API's file list.
///
/// `--name-status` is the driver, because it names every changed path and its
/// order is the one the panel shows; `--numstat` only supplies the counts. A
/// path with no `numstat` record — git prints none for a mode-only change of
/// an empty file, for instance — reports zeroes rather than being dropped.
fn build_files(name_status: &str, numstat: &str) -> Vec<DiffFile> {
    let counts = parse_numstat(numstat);

    parse_name_status(name_status)
        .into_iter()
        .map(|(status, path)| {
            let (additions, deletions) = counts.get(&path).copied().unwrap_or((0, 0));
            DiffFile {
                path,
                status,
                additions,
                deletions,
            }
        })
        .collect()
}

/// Parse `git diff --numstat -z` into `path -> (additions, deletions)`.
///
/// Each record is `<added>\t<removed>\t<path>` followed by a NUL. `-z` rather
/// than the default because git quotes unusual path names in its ordinary
/// output, and a quoted path would not compare equal to the one
/// `--name-status` reported.
fn parse_numstat(raw: &str) -> HashMap<String, (u32, u32)> {
    let mut counts = HashMap::new();
    let mut fields = raw.split('\0');

    while let Some(record) = fields.next() {
        // The terminator after the last record.
        if record.is_empty() {
            continue;
        }

        let mut parts = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            warn!("skipping a numstat record with an unexpected field count");
            continue;
        };

        // Defensive only, because `--no-renames` is passed: a rename record
        // leaves the path empty and follows it with source and destination as
        // two more NUL-terminated fields.
        let path = if path.is_empty() {
            let (Some(_source), Some(destination)) = (fields.next(), fields.next()) else {
                warn!("skipping a numstat rename record without both of its paths");
                continue;
            };
            destination
        } else {
            path
        };

        counts.insert(path.to_string(), (parse_count(added), parse_count(removed)));
    }

    counts
}

/// One `--numstat` count. `-` is what git prints for a binary file, and
/// anything else unreadable is treated the same way: no counted lines
/// (`SPEC.md`, "Git").
fn parse_count(raw: &str) -> u32 {
    raw.parse().unwrap_or(0)
}

/// Parse `git diff --name-status -z` into `(status, path)` pairs, in git's
/// order.
///
/// The status and the path are two NUL-terminated fields, so an unreadable
/// status still has to consume the path that follows it; otherwise every
/// record after it would be read one field out of step.
fn parse_name_status(raw: &str) -> Vec<(DiffStatus, String)> {
    let mut files = Vec::new();
    let mut fields = raw.split('\0');

    while let Some(field) = fields.next() {
        // The terminator after the last record.
        if field.is_empty() {
            continue;
        }

        let status = field.chars().next().and_then(DiffStatus::parse);

        let Some(path) = fields.next() else {
            warn!("skipping a name-status record without a path");
            break;
        };

        let Some(status) = status else {
            warn!("skipping a name-status record with an unusable status");
            continue;
        };

        // Defensive, for the same reason as in `parse_numstat`: `R` and `C`
        // carry a source and a destination, and the destination is the path
        // the API names.
        let path = if matches!(status, DiffStatus::Renamed | DiffStatus::Copied) {
            let Some(destination) = fields.next() else {
                warn!("skipping a rename record without a destination path");
                break;
            };
            destination
        } else {
            path
        };

        files.push((status, path.to_string()));
    }

    files
}

/// Cut `patch` at the last character boundary at or before `limit` bytes.
///
/// The second half of the pair is `SPEC.md`'s `truncated`. A patch of exactly
/// `limit` bytes is returned whole: the limit is what is allowed, not what is
/// exceeded.
fn truncate_patch(patch: String, limit: usize) -> (String, bool) {
    if patch.len() <= limit {
        return (patch, false);
    }

    let mut end = limit;
    // `is_char_boundary(0)` is always true, so this terminates.
    while !patch.is_char_boundary(end) {
        end -= 1;
    }

    let mut patch = patch;
    patch.truncate(end);

    (patch, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Join `records` the way git's `-z` output arrives: a NUL after each one,
    /// including the last. Spelled out rather than written as a literal
    /// because a NUL escape followed by a digit is unreadable.
    fn nul_terminated(records: &[&str]) -> String {
        records.iter().map(|record| format!("{record}\0")).collect()
    }

    #[test]
    fn the_two_listings_are_joined_by_path_in_name_status_order() {
        // The three-file change the integration tests produce.
        let name_status = nul_terminated(&["M", "README.md", "A", "a.txt", "D", "old.txt"]);
        let numstat = nul_terminated(&["1\t0\tREADME.md", "1\t0\ta.txt", "0\t1\told.txt"]);

        let files = build_files(&name_status, &numstat);

        assert_eq!(
            files,
            vec![
                DiffFile {
                    path: "README.md".to_string(),
                    status: DiffStatus::Modified,
                    additions: 1,
                    deletions: 0,
                },
                DiffFile {
                    path: "a.txt".to_string(),
                    status: DiffStatus::Added,
                    additions: 1,
                    deletions: 0,
                },
                DiffFile {
                    path: "old.txt".to_string(),
                    status: DiffStatus::Deleted,
                    additions: 0,
                    deletions: 1,
                },
            ]
        );
    }

    #[test]
    fn an_empty_diff_lists_no_files() {
        assert!(build_files("", "").is_empty());
    }

    #[test]
    fn a_binary_file_reports_no_counted_lines() {
        let files = build_files(
            &nul_terminated(&["M", "blob.bin"]),
            &nul_terminated(&["-\t-\tblob.bin"]),
        );

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].additions, 0);
        assert_eq!(files[0].deletions, 0);
    }

    #[test]
    fn a_path_with_a_tab_or_a_newline_in_it_survives_the_nul_parsing() {
        let path = "src/odd\tname\nwith newlines.rs";
        let files = build_files(
            &nul_terminated(&["A", path]),
            &nul_terminated(&[&format!("7\t0\t{path}")]),
        );

        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].path, path);
        assert_eq!(files[0].additions, 7);
    }

    #[test]
    fn a_path_without_a_numstat_record_still_appears_with_zero_counts() {
        let files = build_files(&nul_terminated(&["T", "link"]), "");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, DiffStatus::TypeChanged);
        assert_eq!((files[0].additions, files[0].deletions), (0, 0));
    }

    #[test]
    fn an_unusable_status_is_skipped_without_shifting_the_records_after_it() {
        // `U` cannot occur between two commits; if it did, the record after it
        // must still be read correctly.
        let files = build_files(
            &nul_terminated(&["U", "conflicted.rs", "A", "a.txt"]),
            &nul_terminated(&["1\t0\ta.txt"]),
        );

        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(files[0].status, DiffStatus::Added);
    }

    #[test]
    fn a_rename_record_is_reported_at_its_destination() {
        // Not produced while `--no-renames` is passed; the parsers handle it
        // so that turning rename detection on later could not silently mispair
        // the two listings.
        let files = build_files(
            &nul_terminated(&["R100", "old.rs", "new.rs"]),
            &nul_terminated(&["0\t0\t", "old.rs", "new.rs"]),
        );

        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].path, "new.rs");
        assert_eq!(files[0].status, DiffStatus::Renamed);
    }

    #[test]
    fn a_patch_at_or_below_the_limit_is_returned_whole() {
        for patch in ["", "diff --git a/a b/a\n"] {
            let (kept, truncated) = truncate_patch(patch.to_string(), patch.len());

            assert_eq!(kept, patch);
            assert!(!truncated, "a patch of exactly the limit was truncated");
        }
    }

    #[test]
    fn a_longer_patch_is_cut_at_the_limit() {
        let (kept, truncated) = truncate_patch("abcdefgh".to_string(), 3);

        assert_eq!(kept, "abc");
        assert!(truncated);
    }

    #[test]
    fn a_cut_never_lands_inside_a_character() {
        // `é` is two bytes, occupying indices 1 and 2, so a limit of 2 falls
        // inside it and the cut has to walk back to index 1.
        let (kept, truncated) = truncate_patch("aéb".to_string(), 2);

        assert_eq!(kept, "a");
        assert!(truncated);
        // The point of the boundary walk: the result is still a `String`, and
        // it is a prefix of the original.
        assert!("aéb".starts_with(&kept));
    }

    #[test]
    fn a_limit_that_falls_inside_the_first_character_gives_an_empty_patch() {
        let (kept, truncated) = truncate_patch("é".to_string(), 1);

        assert_eq!(kept, "");
        assert!(truncated);
    }
}
