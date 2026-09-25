//! The first-parent history of an integration head, and the commit ranges it
//! is attributed over (`ARCHITECTURE.md`, "Git model", History; `SPEC.md`,
//! "Git": `GET /projects/{pid}/git/history`).
//!
//! Read-only, like [`super::diff`]: every command runs directly against the
//! project repository on fixed commits, makes no temporary clone, writes no
//! ref and emits no event. The service resolves the head under the project
//! git lock and hands the commit in, so nothing here takes a lock.
//!
//! **One entry, one range.** The history is `git log --first-parent`: one
//! entry per commit the head has pointed at or passed through on its own line.
//! What an entry *brought in* is the range from its first parent to itself —
//! every commit reachable from the entry and not from its first parent
//! ([`entry_range`]). For a merge commit that is the merge plus the side
//! branch it merged; for any other commit, a fast-forwarded one included, it
//! is the commit alone. Every commit reachable from the head therefore belongs
//! to exactly one entry's range, which is what lets a hand-off commit be
//! attributed to exactly one entry however it arrived.
//!
//! [`range_commits`] is the same question over any range, `exclude..head`:
//! what a revert to a point would take back is `range_commits(head,
//! Some(point))`, and [`first_parent_range`] is that range's own first-parent
//! line, the entries a revert lists (`ARCHITECTURE.md`, "Git model", Revert).

use std::path::Path;

use chrono::{DateTime, Utc};

use super::{GitCommand, GitError};
use crate::models::is_commit_id;
use crate::prelude::*;

/// The fields of one entry, NUL-separated, in the order [`parse_log`] reads
/// them: commit, parents, author name, committer date, the `Requested-By`
/// trailer values and the subject.
///
/// With `-z` every record is NUL-terminated too, so the whole output is a flat
/// sequence of NUL-terminated fields read [`FIELDS`] at a time. NUL is the one
/// byte no commit message, author name or trailer can contain. The trailer
/// values are joined by `\x01`; `%(trailers:key=…,valueonly,separator=…)` is
/// git 2.25, well under the supported floor of 2.39
/// (`ARCHITECTURE.md`, "Git model", Supported git).
const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%cI%x00%(trailers:key=Requested-By,valueonly,separator=%x01)%x00%s";

/// How many fields [`LOG_FORMAT`] prints per commit.
const FIELDS: usize = 6;

/// One commit on an integration head's first-parent line, as git reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// The commit's full object id.
    pub commit: String,
    /// Its parents, first parent first. Empty for a root commit.
    pub parents: Vec<String>,
    /// The first line of its message.
    pub subject: String,
    /// The author's name.
    pub author_name: String,
    /// The committer date.
    pub committed_at: DateTime<Utc>,
    /// The last `Requested-By` trailer value, if the message has one: what
    /// every commit the orchestrator makes carries (`ARCHITECTURE.md`, "Git
    /// model", Commit identity).
    pub requested_by: Option<String>,
}

/// One page of the first-parent history of `head`, newest first.
///
/// Without `before`, the page starts at `head`. With it, `before` has to be a
/// commit on `head`'s first-parent line — the last entry of the previous page
/// — and the page starts at the entry after it; anything else, a commit that
/// is not in the repository or one that only a side branch reaches, is
/// [`GitError::UnknownRef`] (400), because a page walked from it would not be
/// a page of this history. `limit` is the caller's, already bounded.
pub async fn first_parent_page(
    repo: &Path,
    head: &str,
    before: Option<&str>,
    limit: u32,
) -> std::result::Result<Vec<LogEntry>, GitError> {
    let start = match before {
        Some(before) => {
            require_on_first_parent_line(repo, head, before).await?;
            before
        }
        None => head,
    };

    let max_count = format!("--max-count={limit}");
    let mut args = vec!["log", "--first-parent", "-z", LOG_FORMAT, &max_count];
    if before.is_some() {
        // The cursor itself was the previous page's last entry.
        args.push("--skip=1");
    }
    args.extend(["--end-of-options", start]);

    let output = GitCommand::new().args(&args).cwd(repo).run_ok().await?;

    Ok(parse_log(&output.stdout))
}

/// Every entry of `head`'s first-parent line down to, and not including,
/// `exclude`, newest first: the first-parent range `exclude..head`.
///
/// What a revert to `exclude` takes back, entry by entry (`ARCHITECTURE.md`,
/// "Git model", Revert). The caller has established that `exclude` is on
/// `head`'s first-parent line ([`require_on_first_parent_line`]); for any
/// other commit `git log --first-parent <head> ^<exclude>` would still answer,
/// but with a line that does not end where the caller thinks it does. Both
/// are full object ids, checked again because they reach argv.
pub async fn first_parent_range(
    repo: &Path,
    head: &str,
    exclude: &str,
) -> std::result::Result<Vec<LogEntry>, GitError> {
    require_commit_id(head)?;
    require_commit_id(exclude)?;
    let excluded = format!("^{exclude}");

    let output = GitCommand::new()
        .args([
            "log",
            "--first-parent",
            "-z",
            LOG_FORMAT,
            "--end-of-options",
            head,
            &excluded,
        ])
        .cwd(repo)
        .run_ok()
        .await?;

    Ok(parse_log(&output.stdout))
}

/// The range a first-parent entry brought in: every commit reachable from
/// `entry` and not from `parents[0]`, or `entry` alone when it has at most one
/// parent (see the module documentation).
pub async fn entry_range(
    repo: &Path,
    entry: &LogEntry,
) -> std::result::Result<Vec<String>, GitError> {
    if entry.parents.len() < 2 {
        return Ok(vec![entry.commit.clone()]);
    }

    range_commits(
        repo,
        &entry.commit,
        entry.parents.first().map(String::as_str),
    )
    .await
}

/// Every commit reachable from `head` and not from `exclude`: `git rev-list
/// <head> ^<exclude>`, the whole of `head` when `exclude` is `None`.
///
/// Both are full object ids the caller resolved; they are checked again here
/// because they reach argv, and a `^` is prepended to one.
pub async fn range_commits(
    repo: &Path,
    head: &str,
    exclude: Option<&str>,
) -> std::result::Result<Vec<String>, GitError> {
    require_commit_id(head)?;
    let excluded = exclude
        .map(|commit| require_commit_id(commit).map(|()| format!("^{commit}")))
        .transpose()?;

    let mut args = vec!["rev-list", "--end-of-options", head];
    if let Some(excluded) = excluded.as_deref() {
        args.push(excluded);
    }

    let output = GitCommand::new().args(&args).cwd(repo).run_ok().await?;

    Ok(output
        .stdout
        .lines()
        .filter(|line| is_commit_id(line))
        .map(str::to_string)
        .collect())
}

/// Is `before` on `head`'s first-parent line? [`GitError::UnknownRef`] if not.
///
/// `git rev-list --first-parent <head> ^<each parent of before>` lists the
/// first-parent line from `head` down to `before` when `before` is on it, and
/// stops short of it otherwise: the excluded parents cut the walk off exactly
/// below `before`, so the answer costs the distance to the cursor and not the
/// length of the whole history.
///
/// `pub(crate)` for the revert, whose `to` has to be on the same line
/// ([`super::revert`]).
pub(crate) async fn require_on_first_parent_line(
    repo: &Path,
    head: &str,
    before: &str,
) -> std::result::Result<(), GitError> {
    let unknown = || GitError::UnknownRef(before.to_string());
    if !is_commit_id(before) {
        return Err(unknown());
    }
    require_commit_id(head)?;

    // `rev-list --parents -n 1 <before>`: the commit followed by its parents,
    // or a failure when the object is not a commit in this repository.
    let parents = GitCommand::new()
        .args([
            "rev-list",
            "--parents",
            "--max-count=1",
            "--end-of-options",
            before,
        ])
        .cwd(repo)
        .run()
        .await?;
    if parents.status != 0 {
        return Err(unknown());
    }
    let mut ids = parents.stdout.split_whitespace();
    if ids.next() != Some(before) {
        return Err(unknown());
    }
    let excluded: Vec<String> = ids.map(|parent| format!("^{parent}")).collect();

    let mut args = vec!["rev-list", "--first-parent", "--end-of-options", head];
    args.extend(excluded.iter().map(String::as_str));
    let line = GitCommand::new().args(&args).cwd(repo).run_ok().await?;

    if line.stdout.lines().any(|commit| commit == before) {
        Ok(())
    } else {
        Err(unknown())
    }
}

/// A full object id or [`GitError::InvalidRef`]: what may reach argv here.
fn require_commit_id(commit: &str) -> std::result::Result<(), GitError> {
    if is_commit_id(commit) {
        Ok(())
    } else {
        Err(GitError::InvalidRef(commit.to_string()))
    }
}

/// The entries [`LOG_FORMAT`] printed, in git's order.
///
/// A record that does not parse — a date git printed in a form this cannot
/// read, an id that is not a full object id — is skipped with a `warn!`
/// rather than failing the page, as the ref listings do.
fn parse_log(stdout: &str) -> Vec<LogEntry> {
    let fields: Vec<&str> = stdout.split('\0').collect();
    let mut entries = Vec::new();

    // `split` leaves one empty string after the final terminator; `as_chunks`
    // drops it together with any other incomplete tail.
    for [commit, parents, author, date, trailers, subject] in fields.as_chunks::<FIELDS>().0 {
        // A stray newline between records is tolerated rather than trusted
        // never to appear.
        let commit = commit.trim();

        if !is_commit_id(commit) {
            warn!("skipping a log record without a full object id");
            continue;
        }
        let Ok(committed_at) = DateTime::parse_from_rfc3339(date) else {
            warn!(commit = %commit, "skipping a log record with an unreadable committer date");
            continue;
        };

        entries.push(LogEntry {
            commit: commit.to_string(),
            parents: parents.split_whitespace().map(str::to_string).collect(),
            subject: subject.to_string(),
            author_name: author.to_string(),
            committed_at: committed_at.with_timezone(&Utc),
            requested_by: trailers
                .split('\u{1}')
                .map(str::trim)
                .rfind(|value| !value.is_empty())
                .map(str::to_string),
        });
    }

    entries
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::git::testutil::run_git;

    /// A bare repository standing in for a project repository, with a work
    /// clone to build history in, both removed on drop.
    struct Repo {
        dir: TempDir,
    }

    impl Repo {
        async fn create() -> Self {
            let dir = tempfile::tempdir().expect("a temporary directory");
            run_git(
                dir.path(),
                &[
                    "init",
                    "--bare",
                    "--quiet",
                    "--initial-branch=main",
                    "repo.git",
                ],
            )
            .await;
            run_git(
                dir.path(),
                &["init", "--quiet", "--initial-branch=main", "work"],
            )
            .await;
            Self { dir }
        }

        fn bare(&self) -> PathBuf {
            self.dir.path().join("repo.git")
        }

        fn work(&self) -> PathBuf {
            self.dir.path().join("work")
        }

        /// One empty commit on the work clone's current branch.
        async fn commit(&self, message: &str) -> String {
            run_git(
                &self.work(),
                &["commit", "--quiet", "--allow-empty", "-m", message],
            )
            .await;
            self.head().await
        }

        async fn head(&self) -> String {
            run_git(&self.work(), &["rev-parse", "HEAD"])
                .await
                .trim()
                .to_string()
        }

        /// Push the work clone's `main` into the bare repository.
        async fn publish(&self) {
            let bare = self.bare();
            let bare = bare.to_str().expect("a UTF-8 path");
            run_git(
                &self.work(),
                &["push", "--quiet", "--force", bare, "HEAD:refs/heads/main"],
            )
            .await;
        }
    }

    fn commits(entries: &[LogEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.commit.as_str()).collect()
    }

    /// Main: A — B — M(B, S2) — C, with S1 — S2 branched from A and merged
    /// with a `Requested-By` trailer. Answers every id in that order.
    async fn history_with_a_merge(repo: &Repo) -> [String; 6] {
        let a = repo.commit("a").await;
        run_git(&repo.work(), &["checkout", "--quiet", "-b", "side"]).await;
        let s1 = repo.commit("s1").await;
        let s2 = repo.commit("s2").await;
        run_git(&repo.work(), &["checkout", "--quiet", "main"]).await;
        let b = repo.commit("b").await;
        run_git(
            &repo.work(),
            &[
                "merge",
                "--quiet",
                "--no-ff",
                "-m",
                "Merge side into main\n\nRequested-By: session:fake-session",
                "side",
            ],
        )
        .await;
        let m = repo.head().await;
        let c = repo.commit("c").await;
        repo.publish().await;

        [a, s1, s2, b, m, c]
    }

    #[tokio::test]
    async fn the_walk_follows_first_parents_only_newest_first() {
        let repo = Repo::create().await;
        let [a, _s1, s2, b, m, c] = history_with_a_merge(&repo).await;

        let page = first_parent_page(&repo.bare(), &c, None, 50)
            .await
            .expect("the history reads");

        assert_eq!(commits(&page), [c.as_str(), &m, &b, &a]);

        let merge = &page[1];
        assert_eq!(merge.parents, [b.clone(), s2.clone()]);
        assert_eq!(merge.subject, "Merge side into main");
        assert_eq!(merge.requested_by.as_deref(), Some("session:fake-session"));
        assert_eq!(merge.author_name, crate::git::testutil::TEST_AUTHOR_NAME);

        assert_eq!(page[0].requested_by, None);
        assert_eq!(page[0].subject, "c");
        assert!(page[3].parents.is_empty(), "the root has no parents");
    }

    #[tokio::test]
    async fn a_page_continues_after_its_cursor() {
        let repo = Repo::create().await;
        let [a, s1, _s2, b, m, c] = history_with_a_merge(&repo).await;
        let bare = repo.bare();

        let first = first_parent_page(&bare, &c, None, 2).await.expect("page 1");
        assert_eq!(commits(&first), [c.as_str(), &m]);

        let second = first_parent_page(&bare, &c, Some(&m), 2)
            .await
            .expect("page 2");
        assert_eq!(commits(&second), [b.as_str(), &a]);

        let last = first_parent_page(&bare, &c, Some(&a), 2)
            .await
            .expect("the page after the root");
        assert!(last.is_empty());

        // A commit only the side branch reaches is not a cursor of this
        // history, and neither is one the repository does not have.
        for cursor in [
            s1.as_str(),
            "0123456789abcdef0123456789abcdef01234567",
            "main",
        ] {
            let error = first_parent_page(&bare, &c, Some(cursor), 2)
                .await
                .expect_err("the cursor is refused");
            assert!(
                matches!(error, GitError::UnknownRef(_)),
                "{cursor}: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_merge_brings_in_its_side_branch_and_any_other_commit_itself() {
        let repo = Repo::create().await;
        let [a, s1, s2, b, m, c] = history_with_a_merge(&repo).await;
        let bare = repo.bare();

        let page = first_parent_page(&bare, &c, None, 50)
            .await
            .expect("the history reads");
        let mut ranges = Vec::new();
        for entry in &page {
            ranges.push(entry_range(&bare, entry).await.expect("the range reads"));
        }

        let mut merge_range = ranges[1].clone();
        merge_range.sort();
        let mut expected = vec![m.clone(), s1.clone(), s2.clone()];
        expected.sort();
        assert_eq!(merge_range, expected);

        assert_eq!(ranges[0], std::slice::from_ref(&c));
        assert_eq!(ranges[2], std::slice::from_ref(&b));
        assert_eq!(ranges[3], std::slice::from_ref(&a));

        // The range helper over an arbitrary point: everything after `b`.
        let mut after_b = range_commits(&bare, &c, Some(&b))
            .await
            .expect("the range reads");
        after_b.sort();
        let mut expected = vec![c, m, s1, s2];
        expected.sort();
        assert_eq!(after_b, expected);
    }

    #[tokio::test]
    async fn a_fast_forward_leaves_every_commit_its_own_entry() {
        let repo = Repo::create().await;
        let a = repo.commit("a").await;
        run_git(&repo.work(), &["checkout", "--quiet", "-b", "side"]).await;
        let s1 = repo.commit("s1").await;
        let s2 = repo.commit("s2").await;
        run_git(&repo.work(), &["checkout", "--quiet", "main"]).await;
        run_git(&repo.work(), &["merge", "--quiet", "--ff-only", "side"]).await;
        repo.publish().await;
        let bare = repo.bare();

        let page = first_parent_page(&bare, &s2, None, 50)
            .await
            .expect("the history reads");
        assert_eq!(commits(&page), [s2.as_str(), &s1, &a]);

        let tip = entry_range(&bare, &page[0]).await.expect("the range reads");
        assert_eq!(tip, [s2]);
    }

    #[test]
    fn a_record_that_does_not_parse_is_skipped() {
        let good = "0123456789abcdef0123456789abcdef01234567";
        let output = format!(
            "not-an-id\0\0x\02026-09-25T10:00:00+00:00\0\0bad\0\
             {good}\0\0Ada\0not a date\0\0bad date\0\
             {good}\0\0Ada\02026-09-25T10:00:00+02:00\0system\u{1}user:x\0good\0"
        );

        let entries = parse_log(&output);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].subject, "good");
        assert_eq!(entries[0].requested_by.as_deref(), Some("user:x"));
        assert_eq!(
            entries[0].committed_at,
            DateTime::parse_from_rfc3339("2026-09-25T08:00:00Z")
                .expect("a date")
                .with_timezone(&Utc)
        );
    }
}
