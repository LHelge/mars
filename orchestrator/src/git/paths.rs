//! [`DataPaths`], the one place that knows the layout of `DATA_DIR`.
//!
//! `ARCHITECTURE.md`, "Storage" draws the tree; this is that tree as code, so
//! the project repository, a session's work clone and the temporary directory
//! are spelled once rather than joined by hand at every call site. Nothing
//! here touches the filesystem: a path is derived, and whoever needs the
//! directory to exist creates it.
//!
//! The paths are the orchestrator's own view of the volume (`DATA_DIR`), not
//! the host view (`DATA_DIR_HOST`) the engine needs for a bind-mount source.
//! The two are the same directory and are equal outside the compose
//! deployment, but only the orchestrator's view may be handed to git: the
//! mirror's absolute path is recorded in a session clone's alternates file and
//! has to resolve inside the container as well (ADR 0001).

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::prelude::*;

/// The `projects/<id>/` subtree, one per project.
const PROJECTS: &str = "projects";
/// The bare project repository inside it, called the "mirror" throughout the
/// documents even though it is not an exact upstream mirror (ADR 0017).
const PROJECT_REPO: &str = "repo.git";
/// The `sessions/<id>/` subtree, one per session.
const SESSIONS: &str = "sessions";
/// A session's git clone inside it, mounted read-write at `/session/work`.
const SESSION_WORK: &str = "work";
/// Temporary clones for merge and rebase, and the temporary credential
/// configs; emptied by orphan cleanup.
const TMP: &str = "tmp";

/// Where everything under `DATA_DIR` lives.
///
/// Cheap to build and to clone, so a caller holds one for the length of an
/// operation rather than threading [`Config`] into the git layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPaths {
    data_dir: PathBuf,
}

impl DataPaths {
    /// Layout rooted at `data_dir`.
    ///
    /// The path is expected to be absolute — [`Config::from_env`] resolves
    /// `DATA_DIR` at startup — because a relative root would follow the
    /// process's working directory into whatever a later `chdir` made of it.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    /// Layout rooted at the configured `DATA_DIR`.
    pub fn from_config(config: &Config) -> Self {
        Self::new(config.data_dir.clone())
    }

    /// The root itself.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// `DATA_DIR/projects/<project_id>/`: the repository, the CLI state
    /// directory and the project's shared directories.
    pub fn project_dir(&self, project_id: Uuid) -> PathBuf {
        self.data_dir.join(PROJECTS).join(project_id.to_string())
    }

    /// `DATA_DIR/projects/<project_id>/repo.git`: the bare project repository.
    pub fn project_repo(&self, project_id: Uuid) -> PathBuf {
        self.project_dir(project_id).join(PROJECT_REPO)
    }

    /// `DATA_DIR/sessions/<session_id>/`: the clone, the agent's `HOME`, the
    /// logs and the MCP config.
    pub fn session_dir(&self, session_id: Uuid) -> PathBuf {
        self.data_dir.join(SESSIONS).join(session_id.to_string())
    }

    /// `DATA_DIR/sessions/<session_id>/work`: the session's git clone.
    pub fn session_work(&self, session_id: Uuid) -> PathBuf {
        self.session_dir(session_id).join(SESSION_WORK)
    }

    /// `DATA_DIR/tmp/`: temporary clones and the temporary credential configs
    /// [`CredentialConfig::write`](super::CredentialConfig::write) creates.
    pub fn tmp(&self) -> PathBuf {
        self.data_dir.join(TMP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed id, so the expectations below can be read at a glance.
    const ID: &str = "11111111-2222-3333-4444-555555555555";

    fn paths() -> DataPaths {
        DataPaths::new("/data")
    }

    #[test]
    fn the_project_paths_are_the_documented_tree() {
        let id = Uuid::parse_str(ID).expect("a fixed uuid");
        let paths = paths();

        assert_eq!(
            paths.project_dir(id),
            PathBuf::from(format!("/data/projects/{ID}"))
        );
        assert_eq!(
            paths.project_repo(id),
            PathBuf::from(format!("/data/projects/{ID}/repo.git"))
        );
    }

    #[test]
    fn the_session_paths_are_the_documented_tree() {
        let id = Uuid::parse_str(ID).expect("a fixed uuid");
        let paths = paths();

        assert_eq!(
            paths.session_dir(id),
            PathBuf::from(format!("/data/sessions/{ID}"))
        );
        assert_eq!(
            paths.session_work(id),
            PathBuf::from(format!("/data/sessions/{ID}/work"))
        );
    }

    #[test]
    fn tmp_is_one_directory_shared_by_every_project() {
        assert_eq!(paths().tmp(), PathBuf::from("/data/tmp"));
    }

    #[test]
    fn two_projects_never_share_a_directory() {
        let paths = paths();
        let one = Uuid::new_v4();
        let two = Uuid::new_v4();

        assert_ne!(paths.project_dir(one), paths.project_dir(two));
        assert!(paths.project_repo(one).starts_with(paths.project_dir(one)));
    }
}
