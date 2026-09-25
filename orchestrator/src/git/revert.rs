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
    let mut message = format!("Revert {branch} to {}\n\n", short(to));
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
