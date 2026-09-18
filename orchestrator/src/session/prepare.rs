//! Launch preparation: the session directory layout, the `mcp.json` document
//! and the token rotation that precedes every actual process launch.
//!
//! `ARCHITECTURE.md`, "Storage" is the tree —
//! `DATA_DIR/sessions/<sid>/{work,home,log/stream.jsonl,log/stderr.log,mcp.json}`
//! — and "MCP design" → "Per-session config file" is the document written into
//! it. [`SessionDirs`] is that tree as code, in both views of the data volume
//! (`DATA_DIR` for the orchestrator and every session container, `DATA_DIR_HOST`
//! for the engine's bind-mount sources), built on [`DataPaths`] so the segments
//! `sessions/<id>` and `work` are spelled in exactly one place.
//!
//! **Order is the contract** (ADR 0029). The hash commits first and the file is
//! written second; they are not one transaction, but no container starts until
//! both are ready, so any failure here returns `Err` and the caller follows the
//! launch-failure path without starting a process. A later attempt generates
//! another token rather than recovering an interrupted preparation, which is why
//! a stale `mcp.json.tmp` needs no cleanup: the next write overwrites it.
//!
//! **Two entry points.** First creation calls [`initial_token`] *before*
//! inserting the session row, so the hash travels in
//! [`NewSession::mcp_token_hash`](crate::models::NewSession) and the raw token
//! travels to the launcher, which calls [`SessionDirs::ensure`] and
//! [`write_mcp_json`]. Every later launch — resume, conversational retry —
//! calls [`rotate_token`], which commits the replacement hash and then rewrites
//! the file. Adoption of an already-running process after an orchestrator
//! restart calls neither: rotating would invalidate the credentials the live
//! process is holding.
//!
//! Nothing here logs the raw token or the contents of the file; the spans carry
//! `session_id` only (rule 3 of `CLAUDE.md`).

use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::git::DataPaths;
use crate::prelude::*;
use crate::repositories::SessionRepository;
use crate::session::token::McpToken;

/// The agent's `HOME`, mounted at `/session/home`.
const HOME: &str = "home";
/// The log directory, mounted at `/session/log`.
const LOG: &str = "log";
/// Native CLI stdout inside it, which the owner tails.
const STREAM_JSONL: &str = "stream.jsonl";
/// Native CLI stderr inside it, kept for diagnostics.
const STDERR_LOG: &str = "stderr.log";
/// The CLI's MCP configuration, mounted read-only at `/session/mcp.json`.
const MCP_JSON: &str = "mcp.json";
/// Where [`write_mcp_json`] writes before renaming over [`MCP_JSON`].
const MCP_JSON_TMP: &str = "mcp.json.tmp";

/// The mode `mcp.json` is created with.
///
/// Not `0600`, even though the file holds a credential: the container reads it
/// as uid 1000, which the uid contract maps to the orchestrator's own uid under
/// rootless Podman but *not* under Docker, where the orchestrator runs as uid
/// 1000 itself (`ARCHITECTURE.md`, "Uid contract"). `0644` is what both cases
/// can read. The protection is the directory: `/data` is orchestrator-owned and
/// no other user has a path to this file.
const MCP_JSON_MODE: u32 = 0o644;

/// Every path one session owns under the data volume, in both views of it.
///
/// Cheap to build and to clone, so a caller makes one for the length of a
/// launch — usually through [`SessionDirs::from_config`] — rather than
/// threading the configuration into the layer that needs a path. Nothing here
/// touches the filesystem except [`SessionDirs::ensure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDirs {
    /// Rooted at `DATA_DIR`: what the orchestrator and the session container
    /// both see.
    paths: DataPaths,
    /// Rooted at `DATA_DIR_HOST`: what the engine is given as a mount source.
    host_paths: DataPaths,
    session_id: Uuid,
}

impl SessionDirs {
    /// The layout of `session_id` in both views: `data_dir` as the orchestrator
    /// sees it, `data_dir_host` as the engine does.
    ///
    /// The two are the same directory and are equal wherever the orchestrator
    /// runs on the host; they differ in the compose deployment, where the
    /// orchestrator sees `/data` (`README.md`, "Configuration").
    pub fn for_session(data_dir: &Path, data_dir_host: &Path, session_id: Uuid) -> Self {
        Self {
            paths: DataPaths::new(data_dir),
            host_paths: DataPaths::new(data_dir_host),
            session_id,
        }
    }

    /// The layout of `session_id` under the configured `DATA_DIR` and
    /// `DATA_DIR_HOST`.
    ///
    /// The one place production code builds one, for the same reason as
    /// [`Config::project_layout`]: [`Config`] is the only type that holds both
    /// views.
    pub fn from_config(config: &Config, session_id: Uuid) -> Self {
        Self::for_session(&config.data_dir, &config.data_dir_host, session_id)
    }

    /// The session this layout belongs to.
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// `DATA_DIR/sessions/<sid>/`.
    pub fn root(&self) -> PathBuf {
        self.paths.session_dir(self.session_id)
    }

    /// `DATA_DIR/sessions/<sid>/work`: the git clone, mounted read-write at
    /// `/session/work`.
    pub fn work(&self) -> PathBuf {
        self.paths.session_work(self.session_id)
    }

    /// `DATA_DIR/sessions/<sid>/home`: the agent's `HOME`.
    pub fn home(&self) -> PathBuf {
        self.root().join(HOME)
    }

    /// `DATA_DIR/sessions/<sid>/log`: the native CLI's output.
    pub fn log(&self) -> PathBuf {
        self.root().join(LOG)
    }

    /// `DATA_DIR/sessions/<sid>/log/stream.jsonl`: native stdout, the
    /// transcript the owner tails (`ARCHITECTURE.md`, "Durability and
    /// recovery").
    pub fn stream_jsonl(&self) -> PathBuf {
        self.log().join(STREAM_JSONL)
    }

    /// `DATA_DIR/sessions/<sid>/log/stderr.log`: native stderr.
    pub fn stderr_log(&self) -> PathBuf {
        self.log().join(STDERR_LOG)
    }

    /// `DATA_DIR/sessions/<sid>/mcp.json`: the file [`write_mcp_json`]
    /// produces.
    pub fn mcp_json(&self) -> PathBuf {
        self.root().join(MCP_JSON)
    }

    /// The host view of [`root`](Self::root).
    pub fn root_host(&self) -> PathBuf {
        self.host_paths.session_dir(self.session_id)
    }

    /// The host view of [`work`](Self::work): a bind-mount source.
    pub fn work_host(&self) -> PathBuf {
        self.host_paths.session_work(self.session_id)
    }

    /// The host view of [`home`](Self::home).
    pub fn home_host(&self) -> PathBuf {
        self.root_host().join(HOME)
    }

    /// The host view of [`log`](Self::log).
    pub fn log_host(&self) -> PathBuf {
        self.root_host().join(LOG)
    }

    /// The host view of [`stream_jsonl`](Self::stream_jsonl).
    pub fn stream_jsonl_host(&self) -> PathBuf {
        self.log_host().join(STREAM_JSONL)
    }

    /// The host view of [`stderr_log`](Self::stderr_log).
    pub fn stderr_log_host(&self) -> PathBuf {
        self.log_host().join(STDERR_LOG)
    }

    /// The host view of [`mcp_json`](Self::mcp_json): the read-only mount
    /// source for `/session/mcp.json`.
    pub fn mcp_json_host(&self) -> PathBuf {
        self.root_host().join(MCP_JSON)
    }

    /// Create `work/`, `home/` and `log/`, and create `log/stream.jsonl` if it
    /// is missing.
    ///
    /// The transcript file is touched rather than left to the CLI so the owner
    /// can open and tail it from the moment the container starts, without a
    /// race against the entrypoint's redirection (`ARCHITECTURE.md`,
    /// "Durability and recovery"). Touched, never truncated: on a relaunch the
    /// committed offsets still index the existing content.
    ///
    /// `work/` is created empty; the git clone fills it. Idempotent, so a
    /// relaunch or a directory left behind by an interrupted preparation is
    /// completed rather than refused.
    ///
    /// No `chown`: the orchestrator creates directories as its own uid, which
    /// is uid 1000 in the session container under both engines
    /// (`ARCHITECTURE.md`, "Uid contract").
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] when the data directory is not writable; the path is
    /// logged and never put in the message.
    pub async fn ensure(&self) -> Result<()> {
        for directory in [self.work(), self.home(), self.log()] {
            tokio::fs::create_dir_all(&directory)
                .await
                .map_err(|err| io_failure(&directory, "create", err))?;
        }

        let stream = self.stream_jsonl();
        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stream)
            .await
        {
            Ok(_) => {}
            Err(err) => return Err(io_failure(&stream, "create", err)),
        }

        debug!(session_id = %self.session_id, "session directories ready");
        Ok(())
    }
}

/// A fresh token for a session that does not exist yet.
///
/// The route calls this *before* inserting the row: the hash goes into
/// [`NewSession::mcp_token_hash`](crate::models::NewSession) and the token
/// itself is handed to the launcher, which writes the file after creating the
/// directories. The initial hash is therefore part of the inserted row and
/// never a second statement (`docs/data-model.md`,
/// `sessions.mcp_token_hash`; ADR 0029).
pub fn initial_token() -> McpToken {
    McpToken::generate()
}

/// The `mcpServers` document, whose serialisation is the file's byte content.
///
/// The struct exists so the key order is the documented one
/// (`ARCHITECTURE.md`, "MCP design" → "Per-session config file"): `type`,
/// `url`, `headers`. A map would order them by whatever the implementation
/// chose.
#[derive(Serialize)]
struct McpConfig<'a> {
    #[serde(rename = "mcpServers")]
    mcp_servers: McpServers<'a>,
}

/// The one server entry, named `mars-orchestrator` rather than `mars` so a
/// repository's own `.mcp.json` is unlikely to shadow it.
#[derive(Serialize)]
struct McpServers<'a> {
    #[serde(rename = "mars-orchestrator")]
    mars_orchestrator: McpServer<'a>,
}

#[derive(Serialize)]
struct McpServer<'a> {
    /// Always `http`: the Streamable HTTP transport.
    #[serde(rename = "type")]
    transport: &'static str,
    /// `MCP_URL` verbatim (`README.md`, "Configuration").
    url: &'a str,
    headers: McpHeaders<'a>,
}

#[derive(Serialize)]
struct McpHeaders<'a> {
    /// `Bearer <token>`: the per-session credential that identifies the session
    /// to the MCP server (`ARCHITECTURE.md`, "Trust boundaries").
    #[serde(rename = "Authorization")]
    authorization: &'a str,
}

/// Write the session's `mcp.json` through a temporary file and an atomic
/// rename.
///
/// `<root>/mcp.json.tmp` is written, `sync_all`ed and renamed over
/// `<root>/mcp.json`, so a reader — including a container that is still running
/// from a previous launch — sees either the old complete file or the new
/// complete file and never a half-written one. An existing `mcp.json` is
/// replaced by the rename; a stale `.tmp` from an interrupted preparation is
/// overwritten by this one and no separate cleanup is needed (ADR 0029).
///
/// The caller must have run [`SessionDirs::ensure`], or at least created the
/// root, first. Neither the token nor the document is logged (rule 3).
///
/// # Errors
///
/// [`Error::Internal`] when the file cannot be written or renamed. The caller
/// must not start a process.
pub async fn write_mcp_json(dirs: &SessionDirs, mcp_url: &str, token: &McpToken) -> Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let authorization = Zeroizing::new(format!("Bearer {}", token.expose()));
    let document = McpConfig {
        mcp_servers: McpServers {
            mars_orchestrator: McpServer {
                transport: "http",
                url: mcp_url,
                headers: McpHeaders {
                    authorization: &authorization,
                },
            },
        },
    };

    // Infallible in practice — the document is three nested structs of strings
    // — but the value is a credential, so the error must not carry it.
    let contents = Zeroizing::new(serde_json::to_vec(&document).map_err(|err| {
        error!(session_id = %dirs.session_id(), error = %err, "mcp config serialisation failed");
        Error::Internal("could not prepare the session MCP config".into())
    })?);

    let tmp = dirs.root().join(MCP_JSON_TMP);
    let target = dirs.mcp_json();

    // `truncate` rather than `create_new`: a `.tmp` left by a crashed
    // preparation belongs to no live process and is replaced. `mode` so the
    // file is never briefly more permissive than the uid contract asks for.
    let write = async {
        use std::os::unix::fs::PermissionsExt as _;

        let mut file = {
            tokio::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(MCP_JSON_MODE)
                .open(&tmp)
                .await?
        };
        file.write_all(&contents).await?;
        file.sync_all().await?;
        // The `mode` above is masked by the process umask, so the documented
        // bits are set again explicitly rather than left to the environment.
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(MCP_JSON_MODE)).await
    };

    if let Err(err) = write.await {
        // Leave the previous `mcp.json` alone: the rename never happened, so a
        // session that had a valid file still has it.
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(io_failure(&tmp, "write", err));
    }

    tokio::fs::rename(&tmp, &target)
        .await
        .map_err(|err| io_failure(&target, "replace", err))?;

    debug!(session_id = %dirs.session_id(), "session mcp config written");
    Ok(())
}

/// Rotate the session's MCP token for an actual relaunch: commit the
/// replacement hash, then rewrite `mcp.json`.
///
/// The order is ADR 0029's: the hash is committed in its own transaction before
/// the file is written and before any container is created, so a process can
/// never hold a token the row does not know. The two are not one transaction;
/// what the contract guarantees is that no process starts until both are ready,
/// which is why every failure below is an `Err` the caller turns into a launch
/// failure rather than a warning.
///
/// The returned token is the only copy of the value now in `mcp.json`; the
/// caller needs it only if it wants to compare or re-write, and dropping it
/// zeroizes it.
///
/// Adoption of an already-running process does **not** call this: it keeps the
/// stored hash and the existing file, or it would invalidate the credentials
/// that process is holding.
///
/// # Errors
///
/// - [`Error::Conflict`] when the session is not `parked`, `creating` or
///   `failed` — including a `running` session, whose hash and file are left
///   untouched. A session that is gone is folded into the same conflict: it is
///   not relaunchable either.
/// - [`Error::Internal`] when the update or the file write fails.
pub async fn rotate_token(
    pool: &PgPool,
    dirs: &SessionDirs,
    mcp_url: &str,
    session_id: Uuid,
) -> Result<McpToken> {
    let token = McpToken::generate();

    let mut tx = pool.begin().await?;
    let replaced = SessionRepository::new(pool)
        .set_mcp_token_hash_if_relaunchable(&mut tx, session_id, &token.hash())
        .await?;
    if !replaced {
        // No commit of a partial rotation: the transaction rolls back and the
        // stored hash — and therefore the credential the live process holds —
        // is exactly what it was.
        return Err(Error::Conflict("session is not relaunchable".into()));
    }
    tx.commit().await?;

    write_mcp_json(dirs, mcp_url, &token).await?;

    Ok(token)
}

/// A filesystem failure over a session directory: the path goes to the log, a
/// generic message goes to the caller (`CLAUDE.md`, "Backend conventions").
fn io_failure(path: &Path, operation: &'static str, err: std::io::Error) -> Error {
    error!(
        path = %path.display(),
        operation,
        error = %err,
        "session directory operation failed"
    );
    Error::Internal(format!("could not {operation} a session file"))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// A fixed id, so the expectations below can be read at a glance.
    const ID: &str = "11111111-2222-3333-4444-555555555555";

    fn id() -> Uuid {
        Uuid::parse_str(ID).expect("a fixed uuid")
    }

    fn dirs() -> SessionDirs {
        SessionDirs::for_session(Path::new("/data"), Path::new("/host/data"), id())
    }

    #[test]
    fn the_paths_are_the_documented_tree() {
        let dirs = dirs();

        assert_eq!(dirs.root(), PathBuf::from(format!("/data/sessions/{ID}")));
        assert_eq!(dirs.work(), dirs.root().join("work"));
        assert_eq!(dirs.home(), dirs.root().join("home"));
        assert_eq!(dirs.log(), dirs.root().join("log"));
        assert_eq!(dirs.stream_jsonl(), dirs.root().join("log/stream.jsonl"));
        assert_eq!(dirs.stderr_log(), dirs.root().join("log/stderr.log"));
        assert_eq!(dirs.mcp_json(), dirs.root().join("mcp.json"));
    }

    #[test]
    fn the_host_paths_are_the_same_tree_under_the_host_root() {
        let dirs = dirs();

        assert_eq!(
            dirs.root_host(),
            PathBuf::from(format!("/host/data/sessions/{ID}"))
        );
        for (own, host) in [
            (dirs.work(), dirs.work_host()),
            (dirs.home(), dirs.home_host()),
            (dirs.log(), dirs.log_host()),
            (dirs.stream_jsonl(), dirs.stream_jsonl_host()),
            (dirs.stderr_log(), dirs.stderr_log_host()),
            (dirs.mcp_json(), dirs.mcp_json_host()),
        ] {
            assert_eq!(
                own.strip_prefix("/data").expect("rooted at DATA_DIR"),
                host.strip_prefix("/host/data").expect("rooted at the host")
            );
        }
    }

    #[test]
    fn the_session_directory_is_the_one_project_deletion_removes() {
        assert_eq!(
            dirs().root(),
            crate::projects::session_dir(Path::new("/data"), id())
        );
    }

    #[tokio::test]
    async fn ensure_creates_the_tree_and_touches_the_transcript() {
        let data = TempDir::new().expect("a temporary data directory");
        let dirs = SessionDirs::for_session(data.path(), data.path(), id());

        dirs.ensure().await.expect("the directories are created");

        assert!(dirs.work().is_dir());
        assert!(dirs.home().is_dir());
        assert!(dirs.log().is_dir());
        assert!(dirs.stream_jsonl().is_file());
        // The clone fills `work/`; preparation leaves it empty.
        let mut entries = std::fs::read_dir(dirs.work()).expect("work is readable");
        assert!(entries.next().is_none(), "work is not empty");
    }

    #[tokio::test]
    async fn ensure_is_idempotent_and_never_truncates_the_transcript() {
        let data = TempDir::new().expect("a temporary data directory");
        let dirs = SessionDirs::for_session(data.path(), data.path(), id());

        dirs.ensure().await.expect("the first call succeeds");
        std::fs::write(dirs.stream_jsonl(), b"{\"type\":\"system\"}\n").expect("a line is written");

        dirs.ensure().await.expect("the second call succeeds");

        let transcript = std::fs::read(dirs.stream_jsonl()).expect("the transcript is readable");
        assert_eq!(transcript, b"{\"type\":\"system\"}\n");
    }

    #[tokio::test]
    async fn an_unwritable_data_directory_is_an_internal_error() {
        use std::os::unix::fs::PermissionsExt as _;

        let data = TempDir::new().expect("a temporary data directory");
        let read_only = data.path().join("read-only");
        std::fs::create_dir(&read_only).expect("the directory is created");
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o500))
            .expect("the mode is set");

        let dirs = SessionDirs::for_session(&read_only, &read_only, id());
        let error = dirs.ensure().await.expect_err("an unwritable root fails");

        assert!(matches!(error, Error::Internal(_)), "was {error:?}");
        // The message names no path.
        assert!(!error.to_string().contains("read-only"), "{error}");

        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o700))
            .expect("the mode is restored so the directory can be removed");
    }

    /// An obviously fake token, so the expected bytes below can be read
    /// (`CLAUDE.md`, rule 3).
    const FAKE_TOKEN: &str = "fake-mcp-token-not-a-credential-0000000000";

    fn fake_token() -> McpToken {
        // The generated shape is asserted in `token.rs`; this needs a fixed
        // value so the expected bytes below can be written out.
        McpToken::from_raw_for_test(FAKE_TOKEN)
    }

    #[tokio::test]
    async fn the_config_is_the_documented_document_byte_for_byte() {
        let data = TempDir::new().expect("a temporary data directory");
        let dirs = SessionDirs::for_session(data.path(), data.path(), id());
        dirs.ensure().await.expect("the directories are created");

        write_mcp_json(&dirs, "http://orchestrator:7001/mcp", &fake_token())
            .await
            .expect("the config is written");

        let written = std::fs::read_to_string(dirs.mcp_json()).expect("the config is readable");
        assert_eq!(
            written,
            format!(
                "{{\"mcpServers\":{{\"mars-orchestrator\":{{\"type\":\"http\",\
                 \"url\":\"http://orchestrator:7001/mcp\",\
                 \"headers\":{{\"Authorization\":\"Bearer {FAKE_TOKEN}\"}}}}}}}}"
            )
        );
    }

    #[tokio::test]
    async fn a_successful_write_leaves_no_temporary_file_and_mode_0644() {
        use std::os::unix::fs::PermissionsExt as _;

        let data = TempDir::new().expect("a temporary data directory");
        let dirs = SessionDirs::for_session(data.path(), data.path(), id());
        dirs.ensure().await.expect("the directories are created");

        write_mcp_json(&dirs, "http://orchestrator:7001/mcp", &fake_token())
            .await
            .expect("the config is written");

        assert!(!dirs.root().join(MCP_JSON_TMP).exists(), "a .tmp is left");
        let mode = std::fs::metadata(dirs.mcp_json())
            .expect("the config is there")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, MCP_JSON_MODE, "mode was {mode:04o}");
    }

    #[tokio::test]
    async fn a_second_write_replaces_the_previous_config_and_any_stale_temporary() {
        let data = TempDir::new().expect("a temporary data directory");
        let dirs = SessionDirs::for_session(data.path(), data.path(), id());
        dirs.ensure().await.expect("the directories are created");

        write_mcp_json(&dirs, "http://first.test/mcp", &fake_token())
            .await
            .expect("the first config is written");
        // What a crashed preparation would have left behind.
        std::fs::write(dirs.root().join(MCP_JSON_TMP), b"half-written").expect("a stale tmp");

        write_mcp_json(&dirs, "http://second.test/mcp", &fake_token())
            .await
            .expect("the second config is written");

        let written = std::fs::read_to_string(dirs.mcp_json()).expect("the config is readable");
        assert!(written.contains("http://second.test/mcp"), "{written}");
        assert!(!written.contains("http://first.test/mcp"), "{written}");
        assert!(!dirs.root().join(MCP_JSON_TMP).exists(), "a .tmp is left");
    }

    #[tokio::test]
    async fn no_log_line_carries_the_token_or_the_document() {
        use std::os::unix::fs::PermissionsExt as _;

        let capture = crate::email::capture::CaptureWriter::new();
        let data = TempDir::new().expect("a temporary data directory");
        let dirs = SessionDirs::for_session(data.path(), data.path(), id());

        {
            // Non-`Send` guard, which a current-thread test runtime allows.
            let _guard = tracing::subscriber::set_default(capture.subscriber());

            dirs.ensure().await.expect("the directories are created");
            write_mcp_json(&dirs, "http://orchestrator:7001/mcp", &fake_token())
                .await
                .expect("the config is written");

            // And the failure path, which is the one that logs most.
            std::fs::set_permissions(dirs.root(), std::fs::Permissions::from_mode(0o500))
                .expect("the root is made read-only");
            let _ = write_mcp_json(&dirs, "http://orchestrator:7001/mcp", &fake_token()).await;
            std::fs::set_permissions(dirs.root(), std::fs::Permissions::from_mode(0o700))
                .expect("the mode is restored");
        }

        let logs = capture.contents();
        assert!(
            !logs.is_empty(),
            "nothing was captured, so nothing is proven"
        );
        assert!(!logs.contains(FAKE_TOKEN), "leaked the token: {logs}");
        assert!(!logs.contains("Bearer"), "leaked the header: {logs}");
        assert!(!logs.contains("mcpServers"), "leaked the document: {logs}");
        assert!(
            logs.contains(ID),
            "the session id is the field that is logged"
        );
    }

    #[tokio::test]
    async fn a_failed_write_leaves_the_previous_config_intact() {
        use std::os::unix::fs::PermissionsExt as _;

        let data = TempDir::new().expect("a temporary data directory");
        let dirs = SessionDirs::for_session(data.path(), data.path(), id());
        dirs.ensure().await.expect("the directories are created");
        write_mcp_json(&dirs, "http://first.test/mcp", &fake_token())
            .await
            .expect("the first config is written");
        let before = std::fs::read_to_string(dirs.mcp_json()).expect("the config is readable");

        std::fs::set_permissions(dirs.root(), std::fs::Permissions::from_mode(0o500))
            .expect("the root is made read-only");
        let error = write_mcp_json(&dirs, "http://second.test/mcp", &fake_token())
            .await
            .expect_err("a read-only root fails");
        std::fs::set_permissions(dirs.root(), std::fs::Permissions::from_mode(0o700))
            .expect("the mode is restored");

        assert!(matches!(error, Error::Internal(_)), "was {error:?}");
        let after = std::fs::read_to_string(dirs.mcp_json()).expect("the config is still there");
        assert_eq!(after, before);
    }
}
