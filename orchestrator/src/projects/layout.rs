//! [`ProjectLayout`]: every path a project owns under `DATA_DIR`, and the
//! filesystem operations over them.
//!
//! `ARCHITECTURE.md`, "Storage" is the contract — `repo.git`, `claude/` and
//! `shared/<name>` under `DATA_DIR/projects/<id>/`, the CLI state directory
//! shared by the project's sessions and the shared directories created lazily
//! at launch (ADR 0015; `docs/data-model.md`, `project_shared_dirs`). The
//! clone job, project deletion and the shared-directory endpoints all go
//! through this one helper, so a path is joined in exactly one place.
//!
//! **Built on [`DataPaths`]**, which already knows `projects/<id>` and
//! `repo.git` (`crate::git::paths`): this type holds one rather than spelling
//! those segments a second time, and adds the two the git layer has no use
//! for, `claude/` and `shared/`.
//!
//! **Two views of the same directory.** The orchestrator's own view is
//! `DATA_DIR`; the engine needs the host view, `DATA_DIR_HOST`, as the source
//! of every bind mount (`README.md`, "Configuration"). The `_host` variants
//! below are the same layout rooted at the other path — a second [`DataPaths`]
//! rather than a second set of joins — so the two cannot drift apart. Only
//! the orchestrator's view may be handed to git (ADR 0001); only the host view
//! may be handed to the engine as a mount source.
//!
//! **No locking and no database.** The callers hold the per-project git lock
//! (`ARCHITECTURE.md`, "Git model", Serialization). Removing a project's
//! session directories is the Session lifecycle epic's; [`session_dir`] is
//! here so both epics name the same path.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::git::DataPaths;
use crate::models::{SharedDirError, is_shared_dir_name};
use crate::prelude::*;

/// The CLI state directory (`CLAUDE_CONFIG_DIR`), shared by the project's
/// sessions (ADR 0015).
const CLAUDE: &str = "claude";
/// The parent of the project's shared directories.
const SHARED: &str = "shared";
/// The CLI's own per-working-directory subtree inside its state directory.
const CLI_PROJECTS: &str = "projects";
/// How the CLI encodes the working directory every session runs with,
/// `/session/work` (`ARCHITECTURE.md`, "Storage", "Claude Code invocation").
const CLI_WORK_DIR: &str = "-session-work";

/// Where one project's files live, in both views of the data volume.
///
/// Cheap to build and to clone, so a caller makes one for the length of an
/// operation — usually through [`Config::project_layout`] — rather than
/// threading the configuration through the layer that needs a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLayout {
    /// Rooted at `DATA_DIR`: what the orchestrator and every session container
    /// see.
    paths: DataPaths,
    /// Rooted at `DATA_DIR_HOST`: what the engine is given as a mount source.
    host_paths: DataPaths,
    project_id: Uuid,
}

impl ProjectLayout {
    /// The layout of `project_id` under `data_dir`, with the host view equal
    /// to it.
    ///
    /// Equal is correct wherever the orchestrator runs on the host, which is
    /// the development default and what `README.md`, "Configuration" says of
    /// `DATA_DIR_HOST`. A deployment where the two differ — compose, where the
    /// orchestrator sees `/data` — must build the layout from the
    /// configuration instead, through [`Config::project_layout`] or
    /// [`ProjectLayout::new_with_host`], or the `_host` paths below would name
    /// a directory that does not exist on the host.
    pub fn new(data_dir: &Path, project_id: Uuid) -> Self {
        Self::new_with_host(data_dir, data_dir, project_id)
    }

    /// The layout of `project_id` in both views: `data_dir` as the
    /// orchestrator sees it, `data_dir_host` as the engine does.
    pub fn new_with_host(data_dir: &Path, data_dir_host: &Path, project_id: Uuid) -> Self {
        Self {
            paths: DataPaths::new(data_dir),
            host_paths: DataPaths::new(data_dir_host),
            project_id,
        }
    }

    /// The project this layout belongs to.
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }

    /// `DATA_DIR/projects/<id>/`.
    pub fn root(&self) -> PathBuf {
        self.paths.project_dir(self.project_id)
    }

    /// `DATA_DIR/projects/<id>/repo.git`: the bare project repository.
    pub fn repo_git(&self) -> PathBuf {
        self.paths.project_repo(self.project_id)
    }

    /// `DATA_DIR/projects/<id>/claude`: the CLI state directory.
    pub fn claude_dir(&self) -> PathBuf {
        self.root().join(CLAUDE)
    }

    /// `DATA_DIR/projects/<id>/claude/projects/-session-work`: where the CLI
    /// files one transcript per session of this project (`ARCHITECTURE.md`,
    /// "Storage").
    pub fn cli_transcripts_dir(&self) -> PathBuf {
        self.claude_dir().join(CLI_PROJECTS).join(CLI_WORK_DIR)
    }

    /// The transcript file and the directory of the same name a CLI session id
    /// owns, which deleting a session removes (`ARCHITECTURE.md`, "Storage").
    ///
    /// `None` for an id that is not a single plain path component. The value
    /// comes from the CLI's own `init` event, which is agent-controlled output
    /// (`ARCHITECTURE.md`, "Launch sequence"), so one carrying a separator or
    /// `..` would name a path outside the state directory; the caller skips the
    /// removal rather than following it. `<id>.jsonl` first, then `<id>/`.
    pub fn cli_transcript_paths(&self, cli_session_id: &str) -> Option<(PathBuf, PathBuf)> {
        if cli_session_id.is_empty()
            || !cli_session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }

        let root = self.cli_transcripts_dir();
        Some((
            root.join(format!("{cli_session_id}.jsonl")),
            root.join(cli_session_id),
        ))
    }

    /// `DATA_DIR/projects/<id>/shared`: the parent of every shared directory.
    pub fn shared_root(&self) -> PathBuf {
        self.root().join(SHARED)
    }

    /// `DATA_DIR/projects/<id>/shared/<name>`, for a name that passes
    /// [`is_shared_dir_name`].
    ///
    /// The model validates before the row is written, so a stored name is
    /// already good; checking again here is defence in depth, and the reason
    /// every path that takes a name is fallible. Without it a name that
    /// reached the database another way — a hand-edited row, a future
    /// importer — could carry `..` or a separator and escape the project's
    /// directory on the next `clear`.
    pub fn shared_dir(&self, name: &str) -> Result<PathBuf> {
        Ok(self.shared_root().join(checked(name)?))
    }

    /// The host view of [`root`](Self::root).
    pub fn root_host(&self) -> PathBuf {
        self.host_paths.project_dir(self.project_id)
    }

    /// The host view of [`repo_git`](Self::repo_git): the mount source for the
    /// read-only project repository.
    pub fn repo_git_host(&self) -> PathBuf {
        self.host_paths.project_repo(self.project_id)
    }

    /// The host view of [`claude_dir`](Self::claude_dir).
    pub fn claude_dir_host(&self) -> PathBuf {
        self.root_host().join(CLAUDE)
    }

    /// The host view of [`shared_dir`](Self::shared_dir).
    pub fn shared_dir_host(&self, name: &str) -> Result<PathBuf> {
        Ok(self.root_host().join(SHARED).join(checked(name)?))
    }

    /// Create the project's directory, its CLI state directory and the parent
    /// of its shared directories.
    ///
    /// Not `repo.git`: `git init --bare` creates that
    /// ([`crate::git::mirror::init_project_repo`]). Idempotent, so a layout
    /// left half-built by an interrupted clone is completed rather than
    /// refused — the clone job is retryable (`SPEC.md`, "Projects":
    /// `retry-clone`).
    ///
    /// No `chown`: the orchestrator creates directories as its own uid, which
    /// is uid 1000 in the session container under both engines
    /// (`ARCHITECTURE.md`, "Uid contract").
    pub async fn ensure_created(&self) -> Result<()> {
        for directory in [self.root(), self.claude_dir(), self.shared_root()] {
            create_dir_all(&directory).await?;
        }
        Ok(())
    }

    /// Remove the whole project directory: the repository, the CLI state and
    /// every shared directory (`ARCHITECTURE.md`, "Storage").
    ///
    /// This is the removal [`crate::git::mirror::remove_project_repo`] leaves
    /// to its caller; it happens after every session of the project is gone.
    /// A root that is already missing is a success, so deletion can be retried.
    pub async fn remove_all(&self) -> Result<()> {
        remove_dir_all(&self.root()).await
    }

    /// Create `shared/<name>` if it is missing, for the launcher's lazy
    /// creation (`docs/data-model.md`, `project_shared_dirs`).
    pub async fn ensure_shared_dir(&self, name: &str) -> Result<()> {
        create_dir_all(&self.shared_dir(name)?).await
    }

    /// Remove everything inside `shared/<name>` and leave the directory
    /// itself, which is the `clear` action (`SPEC.md`, "Shared directories").
    ///
    /// Entry by entry rather than remove-and-recreate, so the directory the
    /// caller may already have mounted keeps its inode, and each entry is
    /// removed by its own type: a symlink is unlinked with `remove_file` and
    /// never followed, so nothing outside the directory can be deleted through
    /// one. A missing directory is a success — the row may exist before any
    /// session has launched.
    pub async fn clear_shared_dir(&self, name: &str) -> Result<()> {
        let directory = self.shared_dir(name)?;

        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(io_failure(&directory, "read", err)),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| io_failure(&directory, "read", err))?
        {
            let path = entry.path();
            // `DirEntry::file_type` reports the link itself, not its target.
            let is_dir = entry
                .file_type()
                .await
                .map_err(|err| io_failure(&path, "stat", err))?
                .is_dir();

            if is_dir {
                remove_dir_all(&path).await?;
            } else {
                match tokio::fs::remove_file(&path).await {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(io_failure(&path, "remove", err)),
                }
            }
        }

        info!(project_id = %self.project_id, "shared directory cleared");
        Ok(())
    }

    /// Remove `shared/<name>` and everything in it, which is what deleting the
    /// row does (`ARCHITECTURE.md`, "Storage"). Missing is a success.
    pub async fn remove_shared_dir(&self, name: &str) -> Result<()> {
        remove_dir_all(&self.shared_dir(name)?).await
    }
}

/// `DATA_DIR/sessions/<session_id>/`, the directory project deletion removes
/// for each of the project's sessions.
///
/// Session directories belong to the Session lifecycle epic; this is
/// [`DataPaths::session_dir`] under the name project deletion knows it by, so
/// the two epics cannot disagree about where a session lives.
pub fn session_dir(data_dir: &Path, session_id: Uuid) -> PathBuf {
    DataPaths::new(data_dir).session_dir(session_id)
}

/// `name`, if it is one a shared directory may have.
fn checked(name: &str) -> Result<&str> {
    if is_shared_dir_name(name) {
        Ok(name)
    } else {
        // The model's own message, so the 400 reads the same whether the name
        // was refused on the way in or on the way to the filesystem. It never
        // echoes the name.
        Err(Error::BadRequest(SharedDirError::InvalidName.to_string()))
    }
}

/// `mkdir -p`, idempotent.
async fn create_dir_all(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|err| io_failure(path, "create", err))
}

/// `rm -rf`, with a missing path counting as done.
///
/// `pub(crate)` for [`crate::projects::delete`], which removes one directory
/// per former session of the project and must log and tolerate exactly what
/// the methods above do rather than repeat this three-line `match`.
pub(crate) async fn remove_dir_all(path: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_failure(path, "remove", err)),
    }
}

/// `rm -f`, with a missing path counting as done.
///
/// `pub(crate)` for the session service, which removes a deleted session's CLI
/// transcript from inside this layout (`ARCHITECTURE.md`, "Storage") and owes
/// the same tolerance and the same log line as the methods above.
pub(crate) async fn remove_file(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_failure(path, "remove", err)),
    }
}

/// Log the path and return a message that does not carry it.
///
/// The path is not a secret, but it is an internal detail: a 500 answers
/// `internal error` and the log line is where the operator reads which
/// directory failed (`ARCHITECTURE.md`, "Orchestrator internals", Errors).
///
/// "data directory" rather than "project directory": the session service
/// removes a session's own directory through [`remove_dir_all`] as well, and one
/// message for both keeps the two from drifting.
fn io_failure(path: &Path, operation: &'static str, err: std::io::Error) -> Error {
    error!(
        path = %path.display(),
        operation,
        error = %err,
        "data directory operation failed"
    );
    Error::Internal(format!("could not {operation} a data directory"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;
    use crate::models::MAX_SHARED_DIR_NAME_CHARS;

    /// A fixed id, so the expectations below can be read at a glance.
    const ID: &str = "11111111-2222-3333-4444-555555555555";

    fn id() -> Uuid {
        Uuid::parse_str(ID).expect("a fixed uuid")
    }

    fn layout_in(root: &TempDir) -> ProjectLayout {
        ProjectLayout::new(root.path(), id())
    }

    #[test]
    fn the_paths_are_the_documented_tree() {
        let layout = ProjectLayout::new(Path::new("/data"), id());

        assert_eq!(layout.project_id(), id());
        assert_eq!(layout.root(), PathBuf::from(format!("/data/projects/{ID}")));
        assert_eq!(
            layout.repo_git(),
            PathBuf::from(format!("/data/projects/{ID}/repo.git"))
        );
        assert_eq!(
            layout.claude_dir(),
            PathBuf::from(format!("/data/projects/{ID}/claude"))
        );
        assert_eq!(
            layout.shared_root(),
            PathBuf::from(format!("/data/projects/{ID}/shared"))
        );
        assert_eq!(
            layout.shared_dir("target").unwrap(),
            PathBuf::from(format!("/data/projects/{ID}/shared/target"))
        );
    }

    #[test]
    fn the_cli_transcript_paths_are_the_documented_tree() {
        let layout = ProjectLayout::new(Path::new("/data"), id());

        assert_eq!(
            layout.cli_transcripts_dir(),
            PathBuf::from(format!("/data/projects/{ID}/claude/projects/-session-work")),
        );
        assert_eq!(
            layout.cli_transcript_paths("abc-123_DEF"),
            Some((
                PathBuf::from(format!(
                    "/data/projects/{ID}/claude/projects/-session-work/abc-123_DEF.jsonl"
                )),
                PathBuf::from(format!(
                    "/data/projects/{ID}/claude/projects/-session-work/abc-123_DEF"
                )),
            )),
        );
    }

    /// The id comes from agent-controlled CLI output, so one that is not a
    /// plain path component names nothing at all rather than a path outside the
    /// state directory.
    #[test]
    fn a_cli_session_id_that_is_not_one_path_component_names_nothing() {
        let layout = ProjectLayout::new(Path::new("/data"), id());

        for hostile in ["", "..", "../../etc", "a/b", "a\0b", "a b", "/absolute"] {
            assert_eq!(
                layout.cli_transcript_paths(hostile),
                None,
                "{hostile} must not name a path",
            );
        }
    }

    /// The `repo.git` and `projects/<id>` segments have one definition, in
    /// `git::paths`; this is that agreement asserted rather than assumed.
    #[test]
    fn the_paths_are_the_git_layer_s_paths() {
        let paths = DataPaths::new("/data");
        let layout = ProjectLayout::new(Path::new("/data"), id());

        assert_eq!(layout.root(), paths.project_dir(id()));
        assert_eq!(layout.repo_git(), paths.project_repo(id()));
    }

    #[test]
    fn the_host_paths_are_the_same_tree_under_the_host_root() {
        let layout =
            ProjectLayout::new_with_host(Path::new("/data"), Path::new("/srv/mars/data"), id());

        assert_eq!(
            layout.root_host(),
            PathBuf::from(format!("/srv/mars/data/projects/{ID}"))
        );
        assert_eq!(
            layout.repo_git_host(),
            PathBuf::from(format!("/srv/mars/data/projects/{ID}/repo.git"))
        );
        assert_eq!(
            layout.claude_dir_host(),
            PathBuf::from(format!("/srv/mars/data/projects/{ID}/claude"))
        );
        assert_eq!(
            layout.shared_dir_host("target").unwrap(),
            PathBuf::from(format!("/srv/mars/data/projects/{ID}/shared/target"))
        );

        // The two views differ only in their root: the suffix is what the
        // engine mounts and what the container sees, and it must be identical.
        for (own, host) in [
            (layout.root(), layout.root_host()),
            (layout.repo_git(), layout.repo_git_host()),
            (layout.claude_dir(), layout.claude_dir_host()),
            (
                layout.shared_dir("target").unwrap(),
                layout.shared_dir_host("target").unwrap(),
            ),
        ] {
            assert_eq!(
                own.strip_prefix("/data"),
                host.strip_prefix("/srv/mars/data"),
                "{own:?} and {host:?} disagree below the root"
            );
        }
    }

    #[test]
    fn without_a_host_root_both_views_are_the_data_directory() {
        let layout = ProjectLayout::new(Path::new("/data"), id());
        assert_eq!(layout.root(), layout.root_host());
        assert_eq!(layout.repo_git(), layout.repo_git_host());
    }

    #[test]
    fn the_configuration_builds_both_views() {
        let layout = config().project_layout(id());

        assert_eq!(layout.root(), PathBuf::from(format!("/data/projects/{ID}")));
        assert_eq!(
            layout.root_host(),
            PathBuf::from(format!("/srv/mars/data/projects/{ID}"))
        );
    }

    /// A configuration with the two views of the volume set apart, as compose
    /// has them; every value is obviously fake (rule 3), and both paths are
    /// absolute so the working directory `from_vars` resolves against cannot
    /// reach the assertions above.
    fn config() -> Config {
        let vars: std::collections::HashMap<&str, &str> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "unix:///run/user/1000/podman/podman.sock"),
            ("DATA_DIR", "/data"),
            ("DATA_DIR_HOST", "/srv/mars/data"),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
        ]
        .into_iter()
        .collect();

        Config::from_vars(|name| vars.get(name).map(|value| value.to_string()))
            .expect("a complete configuration")
    }

    #[test]
    fn a_session_directory_is_the_git_layer_s_session_directory() {
        let session_id = Uuid::new_v4();
        assert_eq!(
            session_dir(Path::new("/data"), session_id),
            DataPaths::new("/data").session_dir(session_id)
        );
    }

    #[tokio::test]
    async fn ensure_created_makes_the_layout_and_repeats_harmlessly() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);

        layout.ensure_created().await.expect("creates the layout");
        assert!(layout.root().is_dir());
        assert!(layout.claude_dir().is_dir());
        assert!(layout.shared_root().is_dir());
        // `git init --bare` makes this one, not the layout.
        assert!(!layout.repo_git().exists());

        // A marker, so the second run is seen to leave the contents alone.
        let marker = layout.claude_dir().join("keep");
        tokio::fs::write(&marker, b"x").await.expect("writes");

        layout.ensure_created().await.expect("is idempotent");
        assert!(marker.is_file());
    }

    #[tokio::test]
    async fn ensure_created_completes_a_half_built_layout() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);

        // What an interrupted clone leaves behind: the project directory,
        // nothing under it.
        tokio::fs::create_dir_all(layout.root())
            .await
            .expect("creates the root");

        layout.ensure_created().await.expect("completes the layout");
        assert!(layout.claude_dir().is_dir());
        assert!(layout.shared_root().is_dir());
    }

    #[tokio::test]
    async fn remove_all_removes_the_whole_tree_and_tolerates_a_missing_one() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);

        layout.ensure_created().await.expect("creates the layout");
        tokio::fs::create_dir_all(layout.repo_git())
            .await
            .expect("creates a repository directory");

        layout.remove_all().await.expect("removes the tree");
        assert!(!layout.root().exists());

        layout.remove_all().await.expect("a missing root is fine");
    }

    #[tokio::test]
    async fn ensure_shared_dir_creates_one_lazily() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);

        // No `ensure_created` first: the launcher may be the first to touch
        // the project's `shared/`.
        layout
            .ensure_shared_dir("target")
            .await
            .expect("creates the directory");
        assert!(layout.shared_dir("target").unwrap().is_dir());

        layout
            .ensure_shared_dir("target")
            .await
            .expect("is idempotent");
    }

    #[tokio::test]
    async fn clear_shared_dir_empties_it_without_following_symlinks() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);
        layout.ensure_created().await.expect("creates the layout");
        layout
            .ensure_shared_dir("target")
            .await
            .expect("creates the directory");
        let shared = layout.shared_dir("target").unwrap();

        // A file, a nested directory with a file in it, a dangling symlink and
        // a symlink pointing out of the directory at something that must
        // survive.
        tokio::fs::write(shared.join("artifact"), b"x")
            .await
            .expect("writes a file");
        tokio::fs::create_dir_all(shared.join("debug/deps"))
            .await
            .expect("creates a nested directory");
        tokio::fs::write(shared.join("debug/deps/lib.rlib"), b"x")
            .await
            .expect("writes a nested file");
        symlink("/nowhere/at/all", shared.join("dangling")).expect("creates a dangling symlink");

        let outside = root.path().join("outside");
        tokio::fs::create_dir_all(&outside)
            .await
            .expect("creates a directory outside");
        tokio::fs::write(outside.join("precious"), b"x")
            .await
            .expect("writes a file outside");
        symlink(&outside, shared.join("escape")).expect("creates an escaping symlink");

        layout
            .clear_shared_dir("target")
            .await
            .expect("empties the directory");

        assert!(shared.is_dir(), "the directory itself must survive");
        let mut left = tokio::fs::read_dir(&shared).await.expect("reads");
        assert!(
            left.next_entry().await.expect("reads").is_none(),
            "the directory is not empty"
        );
        assert!(
            outside.join("precious").is_file(),
            "a symlink was followed out of the shared directory"
        );
    }

    #[tokio::test]
    async fn clearing_a_directory_that_was_never_created_is_a_success() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);

        layout
            .clear_shared_dir("target")
            .await
            .expect("a missing directory is fine");
        assert!(!layout.shared_dir("target").unwrap().exists());
    }

    #[tokio::test]
    async fn remove_shared_dir_removes_one_and_tolerates_a_missing_one() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);
        layout
            .ensure_shared_dir("target")
            .await
            .expect("creates the directory");
        tokio::fs::write(layout.shared_dir("target").unwrap().join("artifact"), b"x")
            .await
            .expect("writes a file");

        layout
            .remove_shared_dir("target")
            .await
            .expect("removes the directory");
        assert!(!layout.shared_dir("target").unwrap().exists());
        // The parent stays: other shared directories live there.
        assert!(layout.shared_root().is_dir());

        layout
            .remove_shared_dir("target")
            .await
            .expect("a missing directory is fine");
    }

    #[tokio::test]
    async fn a_name_that_is_not_a_shared_directory_name_is_refused() {
        let root = TempDir::new().expect("a temporary directory");
        let layout = layout_in(&root);

        let too_long = "t".repeat(MAX_SHARED_DIR_NAME_CHARS + 1);
        let refused = [
            "../x", "..", ".", "Bad", "", "a/b", "/etc", ".hidden", &too_long,
        ];

        for name in refused {
            for outcome in [
                layout.shared_dir(name).map(|_| ()),
                layout.shared_dir_host(name).map(|_| ()),
                layout.ensure_shared_dir(name).await,
                layout.clear_shared_dir(name).await,
                layout.remove_shared_dir(name).await,
            ] {
                let error = outcome.expect_err(&format!("accepted {name:?}"));
                assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
                // The model's message, and never the name itself.
                assert_eq!(error.to_string(), SharedDirError::InvalidName.to_string());
            }
        }

        // Nothing was created or removed on the way to the rejection.
        assert!(!layout.root().exists());
    }
}
