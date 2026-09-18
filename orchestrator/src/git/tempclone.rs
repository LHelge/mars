//! [`TempClone`], the throwaway working repository merge and rebase run in.
//!
//! `ARCHITECTURE.md`, "Git model" (Merge, rebase, push): an integration never
//! happens in the project repository itself. It happens in a
//! `git clone --shared` under `DATA_DIR/tmp/`, which is then written back with
//! one explicit refspec, so a conflict, a crash or a cancelled request can
//! leave a half-merged index only in a directory nobody else looks at. The
//! project repository is never left conflicted (ADR 0007).
//!
//! `--shared` is what makes this cheap: the clone's
//! `objects/info/alternates` points at the project repository, so it copies no
//! objects and every commit reachable in the mirror — including the ones under
//! `refs/sessions/*` and `refs/handoffs/*` that an ordinary clone would not
//! even have a ref for — is readable from it. `--no-checkout` is the other
//! half: the caller decides which ref becomes the work tree, and populates it
//! once with a single `checkout -B`.
//!
//! The directory is removed on **every** exit path, because removal is a
//! [`Drop`] rather than a step at the end of a function: an early `?`, a
//! conflict, a cancelled task and a panic all unwind through it. The hourly
//! orphan-cleanup job that empties `DATA_DIR/tmp/` remains the backstop for
//! the one case a destructor cannot cover, a process that is killed outright
//! (`ARCHITECTURE.md`, "Background jobs").

use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{DataPaths, GitCommand, GitError};
use crate::prelude::*;

/// A temporary clone of a project repository, deleted when the value drops.
///
/// Hold it for the length of one integration and let it fall out of scope;
/// there is deliberately no `close`, because nothing about a temporary clone
/// is worth failing an otherwise successful merge over. A removal that fails
/// is logged and left to orphan cleanup.
#[derive(Debug)]
pub struct TempClone {
    /// The clone's directory, `DATA_DIR/tmp/<uuid>`.
    path: PathBuf,
}

impl TempClone {
    /// Clone `repo` into a fresh `DATA_DIR/tmp/<uuid>`.
    ///
    /// `git clone --quiet --no-checkout --shared <repo> <tmp>/<uuid>`: no work
    /// tree yet and no objects of its own. `DATA_DIR/tmp/` is created if it is
    /// missing, so a fresh data directory needs no separate step, and the name
    /// is a fresh UUID, so two integrations of the same project — or of two
    /// projects — can never collide even though only one of them holds any
    /// given project git lock.
    ///
    /// The value is constructed before the clone runs, so a clone that fails
    /// part-way takes whatever it created with it instead of leaving a
    /// half-clone behind for the cleanup job.
    pub async fn create(paths: &DataPaths, repo: &Path) -> std::result::Result<Self, GitError> {
        let tmp = paths.tmp();
        std::fs::create_dir_all(&tmp)?;

        let clone = Self {
            path: tmp.join(Uuid::new_v4().to_string()),
        };

        GitCommand::new()
            .args(["clone", "--quiet", "--no-checkout", "--shared"])
            .arg("--end-of-options")
            .arg(repo)
            .arg(&clone.path)
            .cwd(&tmp)
            .run_ok()
            .await?;

        debug!(path = %clone.path.display(), "temporary git clone created");

        Ok(clone)
    }

    /// The clone's directory: the working directory every command of the
    /// operation runs in.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempClone {
    /// Best effort by necessity: a destructor has nobody to return an error
    /// to. A directory left behind is a wasted inode rather than a
    /// correctness problem — it holds no objects of its own and no credential
    /// — and the hourly orphan cleanup removes it, so a failure is a `warn!`
    /// and nothing more.
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => debug!(path = %self.path.display(), "temporary git clone removed"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warn!(
                path = %self.path.display(),
                error = %err,
                "a temporary git clone could not be deleted"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testutil::TestUpstream;
    use crate::git::{ProjectGitLocks, init_project_repo};
    use crate::models::RemoteUrl;

    /// A project repository to clone from, and the `DATA_DIR` it lives in.
    async fn project() -> (tempfile::TempDir, TestUpstream, DataPaths, PathBuf) {
        let upstream = TestUpstream::create().await;
        let data = tempfile::tempdir().expect("a temporary data directory");
        let paths = DataPaths::new(data.path());
        let guard = ProjectGitLocks::new().lock(Uuid::new_v4()).await;

        init_project_repo(
            &guard,
            &paths,
            &RemoteUrl::local_for_tests(&upstream.path),
            None,
            None,
        )
        .await
        .expect("the project repository is initialised");

        let repo = paths.project_repo(guard.project_id());
        (data, upstream, paths, repo)
    }

    #[tokio::test]
    async fn a_temp_clone_borrows_the_mirror_s_objects_and_has_no_work_tree_yet() {
        let (_data, _upstream, paths, repo) = project().await;

        let clone = TempClone::create(&paths, &repo)
            .await
            .expect("the temporary clone is created");

        assert!(clone.path().starts_with(paths.tmp()));
        assert_eq!(
            std::fs::read_to_string(clone.path().join(".git/objects/info/alternates"))
                .expect("the clone has an alternates file"),
            format!("{}\n", repo.join("objects").display())
        );
        assert!(
            !clone.path().join("README.md").exists(),
            "--no-checkout still populated the work tree"
        );
    }

    #[tokio::test]
    async fn dropping_the_clone_removes_its_directory() {
        let (_data, _upstream, paths, repo) = project().await;

        let path = {
            let clone = TempClone::create(&paths, &repo)
                .await
                .expect("the temporary clone is created");
            clone.path().to_path_buf()
        };

        assert!(!path.exists(), "the temporary clone outlived its guard");
        assert_eq!(
            std::fs::read_dir(paths.tmp())
                .expect("the tmp directory is readable")
                .count(),
            0,
            "something was left in DATA_DIR/tmp"
        );
    }

    #[tokio::test]
    async fn two_clones_of_the_same_repository_do_not_collide() {
        let (_data, _upstream, paths, repo) = project().await;

        let first = TempClone::create(&paths, &repo).await.expect("the first");
        let second = TempClone::create(&paths, &repo).await.expect("the second");

        assert_ne!(first.path(), second.path());
    }

    #[tokio::test]
    async fn a_clone_of_something_that_is_not_a_repository_leaves_nothing_behind() {
        let (_data, _upstream, paths, _repo) = project().await;

        let error = TempClone::create(&paths, &paths.data_dir().join("not-a-repo"))
            .await
            .expect_err("there is nothing there to clone");

        assert!(matches!(error, GitError::Command { .. }), "{error:?}");
        assert_eq!(
            std::fs::read_dir(paths.tmp())
                .expect("the tmp directory is readable")
                .count(),
            0,
            "a failed clone left its directory behind"
        );
    }
}
