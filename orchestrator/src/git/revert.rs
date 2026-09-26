//! Reverting an integration head to an earlier commit of its own first-parent
//! line by writing one new commit (`ARCHITECTURE.md`, "Git model", Revert;
//! `SPEC.md`, "Git": `POST /projects/{pid}/git/revert`; ADR 0053).
//!
//! **Revert, never reset.** The head never moves backwards: the new commit has
//! the head as its only parent and `to`'s tree as its tree, so the head moves
//! forward to a commit whose content is `to`'s. Integration heads only moving
//! forward is what the end-of-session judgement of ADR 0050 relies on, and a
//! forward move needs no force push upstream.
//!
//! **No temporary clone.** Nothing is merged, so there is no index and no work
//! tree to need one: `git commit-tree <to>^{tree} -p <head>` writes the commit
//! object straight into the project repository, and `git update-ref
//! refs/heads/<branch> <new> <head>` moves the head with git's own
//! compare-and-swap. The caller holds the project git lock and resolved `head`
//! under it, so the swap failing is an invariant failure — something outside
//! the lock wrote the ref — and not the caller's "branch has moved", which the
//! service decides before anything is written.
//!
//! Both commands take `--end-of-options` and were verified on git 2.39.5, the
//! supported floor (`ARCHITECTURE.md`, "Git model", Supported git).

use std::path::Path;

use uuid::Uuid;

use super::{CommitIdentity, DataPaths, GitCommand, GitError, ProjectGitGuard, refs};
use crate::models::is_commit_id;
use crate::prelude::*;

/// How many hex digits a commit is abbreviated to in a revert message.
///
/// Fixed rather than asked of `git rev-parse --short`: the message is written
/// once and read for years, and twelve digits stay unambiguous in any
/// repository Mars will hold while seven may not.
pub const SHORT_COMMIT: usize = 12;

/// The first [`SHORT_COMMIT`] digits of `commit`, or all of it when shorter.
pub fn short(commit: &str) -> &str {
    commit.get(..SHORT_COMMIT).unwrap_or(commit)
}

/// A revert subject is `Revert <branch> to <short to>`: these are its fixed
/// parts, which [`revert_message`] writes and [`super::history::revert_target`]
/// reads back. A branch name cannot contain a space, so the parts split it
/// unambiguously.
pub const REVERT_SUBJECT_PREFIX: &str = "Revert ";
/// See [`REVERT_SUBJECT_PREFIX`].
pub const REVERT_SUBJECT_TO: &str = " to ";

/// What a revert whose head already has `to`'s tree is told (409), before the
/// branch and the short `to`: `nothing to revert: main already matches
/// <short>` (`SPEC.md`, "Git").
pub const NOTHING_TO_REVERT: &str = "nothing to revert";

/// Do `head` and `to` have the same tree? A revert between them would write a
/// commit that changes nothing, which the service refuses.
///
/// `git rev-parse --verify --end-of-options <commit>^{tree}` for each
/// ([`refs::peel`]; `--verify` takes one revision, and without it `rev-parse`
/// echoes `--end-of-options` back as output): both are full object ids the
/// caller resolved, checked again because they reach argv. Verified on git
/// 2.39.5.
pub async fn same_tree(repo: &Path, head: &str, to: &str) -> std::result::Result<bool, GitError> {
    let mut trees = Vec::with_capacity(2);
    for commit in [head, to] {
        if !is_commit_id(commit) {
            return Err(GitError::InvalidRef(commit.to_string()));
        }
        let tree = refs::peel(repo, commit, "tree")
            .await?
            .ok_or_else(|| GitError::UnknownRef(commit.to_string()))?;
        trees.push(tree);
    }

    Ok(trees.first() == trees.get(1))
}

/// Write the revert commit of the integration head `branch` (short name) from
/// `head` to `to`'s tree, and move the head to it. Answers the new commit.
///
/// `head` and `to` are full object ids the caller resolved and checked under
/// `guard`: `head` is what `refs/heads/<branch>` points at now, and `to` is on
/// its first-parent line. `message` is the whole commit message, trailer
/// included; `identity` is the bot identity, which authors and commits it.
pub async fn revert_to(
    guard: &ProjectGitGuard,
    paths: &DataPaths,
    branch: &str,
    head: &str,
    to: &str,
    message: &str,
    identity: &CommitIdentity,
) -> std::result::Result<String, GitError> {
    for commit in [head, to] {
        if !is_commit_id(commit) {
            return Err(GitError::InvalidRef(commit.to_string()));
        }
    }

    let repo = paths.project_repo(guard.project_id());
    let tree = format!("{to}^{{tree}}");

    let output = GitCommand::new()
        .args(["commit-tree", "-p", head, "-m", message, "--end-of-options"])
        .arg(&tree)
        .identity(identity)
        .cwd(&repo)
        .run_ok()
        .await?;
    let commit = output.stdout.trim().to_string();
    if !is_commit_id(&commit) {
        return Err(GitError::Command {
            args: vec!["commit-tree".to_string()],
            code: Some(0),
            stderr: "commit-tree printed no object id".to_string(),
        });
    }

    refs::update(&repo, &format!("refs/heads/{branch}"), &commit, Some(head)).await?;

    info!(
        project_id = %guard.project_id(),
        git.branch = %branch,
        git.to = %to,
        commit = %commit,
        "reverted an integration head",
    );

    Ok(commit)
}

/// The revert commit's message (`SPEC.md`, "Git").
///
/// `Revert <branch> to <short to>`, a blank line, one line per reverted
/// first-parent commit — its short id and subject, newest first — another
/// blank line and the `Requested-By` trailer, which is the last paragraph so
/// that git reads it back as one ([`super::history`]).
pub fn revert_message<'a>(
    branch: &str,
    to: &str,
    reverted: impl IntoIterator<Item = (&'a str, &'a str)>,
    requested_by_trailer: &str,
) -> String {
    let mut message = format!(
        "{REVERT_SUBJECT_PREFIX}{branch}{REVERT_SUBJECT_TO}{}\n\n",
        short(to)
    );
    for (commit, subject) in reverted {
        message.push_str(short(commit));
        message.push(' ');
        message.push_str(subject);
        message.push('\n');
    }
    message.push('\n');
    message.push_str(requested_by_trailer);
    message.push('\n');

    message
}

/// The `Requested-By` value a revert names: a user's, always, since reverting
/// is not offered over MCP (ADR 0053).
pub fn requested_by_user(user_id: Uuid) -> String {
    super::requested_by_trailer(&super::GitActor::User(user_id))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::git::testutil::{run_git, test_identity};
    use crate::git::{ProjectGitLocks, history};

    /// A project repository under a `DataPaths` root, with a work clone to
    /// build history in.
    struct Repo {
        _dir: TempDir,
        paths: DataPaths,
        project_id: Uuid,
        work: PathBuf,
    }

    impl Repo {
        async fn create() -> Self {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let paths = DataPaths::new(dir.path());
            let project_id = Uuid::new_v4();
            let repo = paths.project_repo(project_id);
            std::fs::create_dir_all(&repo).expect("the project directory");
            run_git(
                &repo,
                &["init", "--bare", "--quiet", "--initial-branch=main", "."],
            )
            .await;
            let work = dir.path().join("work");
            run_git(
                dir.path(),
                &["init", "--quiet", "--initial-branch=main", "work"],
            )
            .await;

            Self {
                _dir: dir,
                paths,
                project_id,
                work,
            }
        }

        fn bare(&self) -> PathBuf {
            self.paths.project_repo(self.project_id)
        }

        /// Commit `file` with `content` on the work clone's branch.
        async fn commit(&self, file: &str, content: &str) -> String {
            std::fs::write(self.work.join(file), content).expect("the file writes");
            run_git(&self.work, &["add", "--", file]).await;
            run_git(&self.work, &["commit", "--quiet", "-m", file]).await;
            self.rev("HEAD").await
        }

        async fn rev(&self, name: &str) -> String {
            run_git(&self.work, &["rev-parse", name])
                .await
                .trim()
                .to_string()
        }

        async fn publish(&self) {
            let bare = self.bare();
            let bare = bare.to_str().expect("a UTF-8 path");
            run_git(
                &self.work,
                &["push", "--quiet", "--force", bare, "HEAD:refs/heads/main"],
            )
            .await;
        }

        async fn bare_rev(&self, name: &str) -> String {
            run_git(&self.bare(), &["rev-parse", name])
                .await
                .trim()
                .to_string()
        }

        /// Bring the work clone's `main` to the bare repository's, after a
        /// revert wrote there.
        async fn sync(&self) {
            let bare = self.bare();
            let bare = bare.to_str().expect("a UTF-8 path");
            run_git(&self.work, &["fetch", "--quiet", bare, "main"]).await;
            run_git(&self.work, &["reset", "--quiet", "--hard", "FETCH_HEAD"]).await;
        }

        /// Commit `file` on a side branch and merge it into `main` with a
        /// merge commit, published. Answers the merge.
        async fn merge_side(&self, side: &str, file: &str) -> String {
            run_git(&self.work, &["checkout", "--quiet", "-b", side]).await;
            self.commit(file, side).await;
            run_git(&self.work, &["checkout", "--quiet", "main"]).await;
            run_git(
                &self.work,
                &[
                    "merge",
                    "--quiet",
                    "--no-ff",
                    "-m",
                    &format!("Merge {side} into main\n\nRequested-By: session:fake-session"),
                    side,
                ],
            )
            .await;
            self.publish().await;
            self.rev("HEAD").await
        }

        /// Revert the bare `main` to `to` as `revert_locked` would, and bring
        /// the work clone along. Answers the revert commit.
        async fn revert(&self, to: &str) -> String {
            let locks = ProjectGitLocks::new();
            let guard = locks.lock(self.project_id).await;
            let head = self.bare_rev("refs/heads/main").await;
            let range = history::first_parent_range(&self.bare(), &head, to)
                .await
                .expect("the range reads");
            let message = revert_message(
                "main",
                to,
                range
                    .iter()
                    .map(|entry| (entry.commit.as_str(), entry.subject.as_str())),
                &requested_by_user(Uuid::nil()),
            );
            let commit = revert_to(
                &guard,
                &self.paths,
                "main",
                &head,
                to,
                &message,
                &test_identity(),
            )
            .await
            .expect("the revert is written");
            self.sync().await;
            commit
        }

        /// What `reverted_by` marks over one page of `main`, as `(entry,
        /// revert)` pairs in page order; the line is walked from the head to
        /// the page's end as the service walks it.
        async fn marks(&self, before: Option<&str>, limit: u32) -> Vec<(String, Option<String>)> {
            let bare = self.bare();
            let head = self.bare_rev("refs/heads/main").await;
            let page = history::first_parent_page(&bare, &head, before, limit)
                .await
                .expect("the page reads");
            let line = match page.last() {
                Some(last) => history::first_parent_line_through(&bare, &head, last)
                    .await
                    .expect("the line reads"),
                None => Vec::new(),
            };
            let undone = history::reverted_by(&bare, "main", &line)
                .await
                .expect("the marks read");

            page.into_iter()
                .map(|entry| {
                    let by = undone.get(&entry.commit).cloned();
                    (entry.commit, by)
                })
                .collect()
        }
    }

    /// The shape found on a real project: `A — B — C — D (merge) — R1
    /// "Revert main to B" — R2 "Revert main to A" — E (merge)`. Answers
    /// `[A, B, C, D, R1, R2, E]`.
    async fn nested_reverts(repo: &Repo) -> [String; 7] {
        let a = repo.commit("a.txt", "a").await;
        let b = repo.commit("b.txt", "b").await;
        let c = repo.commit("c.txt", "c").await;
        let d = repo.merge_side("side-d", "d.txt").await;
        let r1 = repo.revert(&b).await;
        let r2 = repo.revert(&a).await;
        let e = repo.merge_side("side-e", "e.txt").await;

        [a, b, c, d, r1, r2, e]
    }

    fn by(commit: &str) -> Option<String> {
        Some(commit.to_string())
    }

    #[tokio::test]
    async fn a_revert_marks_every_entry_between_to_and_itself() {
        let repo = Repo::create().await;
        let a = repo.commit("a.txt", "a").await;
        let b = repo.commit("b.txt", "b").await;
        let c = repo.commit("c.txt", "c").await;
        repo.publish().await;
        let r = repo.revert(&a).await;

        assert_eq!(
            repo.marks(None, 50).await,
            [
                (r.clone(), None),
                (c, by(&r)),
                (b, by(&r)),
                (a.clone(), None),
            ]
        );
        assert_eq!(
            repo.bare_rev(&format!("{r}^{{tree}}")).await,
            repo.bare_rev(&format!("{a}^{{tree}}")).await,
        );
    }

    #[tokio::test]
    async fn a_later_revert_to_an_older_point_undoes_an_earlier_revert_and_all_it_undid() {
        let repo = Repo::create().await;
        let [a, b, c, d, r1, r2, e] = nested_reverts(&repo).await;

        assert_eq!(
            repo.marks(None, 50).await,
            [
                (e, None),
                (r2.clone(), None),
                (r1, by(&r2)),
                (d, by(&r2)),
                (c, by(&r2)),
                (b, by(&r2)),
                (a, None),
            ]
        );
    }

    #[tokio::test]
    async fn a_revert_back_to_an_undone_commit_brings_what_it_holds_back() {
        let repo = Repo::create().await;
        let a = repo.commit("a.txt", "a").await;
        let b = repo.commit("b.txt", "b").await;
        let c = repo.commit("c.txt", "c").await;
        repo.publish().await;
        let r1 = repo.revert(&a).await;
        // Back to C: R1 is undone, and B and C are the head's content again.
        let r2 = repo.revert(&c).await;

        assert_eq!(
            repo.marks(None, 50).await,
            [
                (r2.clone(), None),
                (r1, by(&r2)),
                (c, None),
                (b, None),
                (a, None),
            ]
        );
    }

    #[tokio::test]
    async fn an_entry_undone_by_a_revert_on_an_earlier_page_is_marked() {
        let repo = Repo::create().await;
        let [a, b, c, d, r1, r2, e] = nested_reverts(&repo).await;

        assert_eq!(
            repo.marks(None, 3).await,
            [(e, None), (r2.clone(), None), (r1.clone(), by(&r2))]
        );
        assert_eq!(
            repo.marks(Some(&r1), 3).await,
            [(d, by(&r2)), (c, by(&r2)), (b.clone(), by(&r2))]
        );
        assert_eq!(repo.marks(Some(&b), 3).await, [(a, None)]);
    }

    #[tokio::test]
    async fn a_revert_to_a_point_below_the_page_marks_the_whole_page() {
        let repo = Repo::create().await;
        let a = repo.commit("a.txt", "a").await;
        repo.commit("b.txt", "b").await;
        let c = repo.commit("c.txt", "c").await;
        let d = repo.commit("d.txt", "d").await;
        repo.publish().await;
        let r = repo.revert(&a).await;

        // The walk ends at C, above A: A is found in the repository instead.
        assert_eq!(
            repo.marks(None, 3).await,
            [(r.clone(), None), (d, by(&r)), (c, by(&r))]
        );
    }

    #[tokio::test]
    async fn a_commit_that_only_reads_like_a_revert_marks_nothing() {
        let repo = Repo::create().await;
        let a = repo.commit("a.txt", "a").await;
        let b = repo.commit("b.txt", "b").await;
        repo.publish().await;
        let subject = format!("Revert main to {}", short(&a));

        // A merge with that subject and a session's trailer.
        run_git(&repo.work, &["checkout", "--quiet", "-b", "side"]).await;
        repo.commit("s.txt", "s").await;
        run_git(&repo.work, &["checkout", "--quiet", "main"]).await;
        run_git(
            &repo.work,
            &[
                "merge",
                "--quiet",
                "--no-ff",
                "-m",
                &format!("{subject}\n\nRequested-By: session:fake-session"),
                "side",
            ],
        )
        .await;
        let merge = repo.rev("HEAD").await;
        // A single-parent commit with A's tree and that subject, but no
        // trailer.
        let bare_commit = run_git(
            &repo.work,
            &[
                "commit-tree",
                "-p",
                &merge,
                "-m",
                &subject,
                &format!("{a}^{{tree}}"),
            ],
        )
        .await
        .trim()
        .to_string();
        run_git(&repo.work, &["reset", "--quiet", "--hard", &bare_commit]).await;
        repo.publish().await;

        assert_eq!(
            repo.marks(None, 50).await,
            [(bare_commit, None), (merge, None), (b, None), (a, None)]
        );
    }

    #[tokio::test]
    async fn same_tree_tells_a_revert_that_would_change_nothing() {
        let repo = Repo::create().await;
        let a = repo.commit("a.txt", "a").await;
        repo.commit("b.txt", "b").await;
        repo.publish().await;
        let r = repo.revert(&a).await;

        assert!(
            same_tree(&repo.bare(), &r, &a)
                .await
                .expect("the trees read")
        );
        let b = repo.bare_rev(&format!("{r}^")).await;
        assert!(
            !same_tree(&repo.bare(), &r, &b)
                .await
                .expect("the trees read")
        );
    }

    #[test]
    fn the_message_lists_the_reverted_commits_before_the_trailer() {
        let head = "a".repeat(40);
        let middle = "b".repeat(40);
        let to = "c".repeat(40);
        let user_id = Uuid::from_u128(7);

        let message = revert_message(
            "main",
            &to,
            [(head.as_str(), "feat: two"), (middle.as_str(), "feat: one")],
            &requested_by_user(user_id),
        );

        assert_eq!(
            message,
            format!(
                "Revert main to cccccccccccc\n\n\
                 aaaaaaaaaaaa feat: two\n\
                 bbbbbbbbbbbb feat: one\n\n\
                 Requested-By: user:{user_id}\n"
            )
        );
    }

    #[tokio::test]
    async fn the_head_moves_forward_to_a_commit_with_to_s_tree() {
        let repo = Repo::create().await;
        let a = repo.commit("a.txt", "a").await;
        let b = repo.commit("b.txt", "b").await;
        let c = repo.commit("a.txt", "changed").await;
        repo.publish().await;
        let locks = ProjectGitLocks::new();
        let guard = locks.lock(repo.project_id).await;

        let reverted = history::first_parent_range(&repo.bare(), &c, &a)
            .await
            .expect("the range reads");
        let message = revert_message(
            "main",
            &a,
            reverted
                .iter()
                .map(|entry| (entry.commit.as_str(), entry.subject.as_str())),
            &requested_by_user(Uuid::nil()),
        );

        let commit = revert_to(
            &guard,
            &repo.paths,
            "main",
            &c,
            &a,
            &message,
            &test_identity(),
        )
        .await
        .expect("the revert is written");

        assert_eq!(repo.bare_rev("refs/heads/main").await, commit);
        assert_eq!(
            repo.bare_rev(&format!("{commit}^{{tree}}")).await,
            repo.bare_rev(&format!("{a}^{{tree}}")).await,
            "the tree is to's"
        );
        assert_eq!(
            repo.bare_rev(&format!("{commit}^")).await,
            c,
            "the head is the only parent"
        );
        assert_ne!(b, commit);

        let log = history::first_parent_page(&repo.bare(), &commit, None, 1)
            .await
            .expect("the log reads");
        assert_eq!(log[0].subject, format!("Revert main to {}", short(&a)));
        assert_eq!(
            log[0].requested_by.as_deref(),
            Some(format!("user:{}", Uuid::nil()).as_str())
        );
        assert_eq!(log[0].author_name, test_identity().name);
    }

    #[tokio::test]
    async fn a_head_that_moved_under_the_lock_is_not_overwritten() {
        let repo = Repo::create().await;
        let a = repo.commit("a.txt", "a").await;
        let b = repo.commit("b.txt", "b").await;
        repo.publish().await;
        let locks = ProjectGitLocks::new();
        let guard = locks.lock(repo.project_id).await;

        // Claimed to be at `a`, which it is not: the swap refuses.
        let error = revert_to(
            &guard,
            &repo.paths,
            "main",
            &a,
            &a,
            "Revert main\n",
            &test_identity(),
        )
        .await
        .expect_err("the stale swap is refused");

        assert!(matches!(error, GitError::Command { .. }), "{error:?}");
        assert_eq!(repo.bare_rev("refs/heads/main").await, b);
    }
}
