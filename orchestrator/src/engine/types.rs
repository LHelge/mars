//! The engine-neutral data types [`ContainerEngine`](super::ContainerEngine)
//! speaks in, plus the label keys and the container naming rule.
//!
//! None of these mention a `bollard` type. That is the whole point of the
//! boundary: `HostConfig` knowledge stays inside `engine/`, so the launcher,
//! the session owner, recovery and the WebSocket terminal describe what they
//! want in these terms and the adapter translates (ADR 0004;
//! `ARCHITECTURE.md`, "Engine adapter").
//!
//! The fields a [`ContainerSpec`] carries are the rows of `ARCHITECTURE.md`,
//! "Session container specification". The fields fixed for every session —
//! `CapDrop`, `SecurityOpt`, `ReadonlyRootfs`, `StdinOnce`, `Tty`,
//! `UsernsMode` — are not fields here: the adapter sets them, because a caller
//! that could vary them could weaken them.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
// Shadows the prelude's one-parameter `Result<T>` alias, so this module can
// write the `Result<T, EngineError>` the engine trait actually returns.
use std::result::Result;

use async_trait::async_trait;
use bytes::Bytes;
use uuid::Uuid;

use super::EngineError;
// The crate convention (`CLAUDE.md`, "Backend conventions"). These are plain
// data types with no crate-wide error of their own to report, so the glob is
// here for the doc links and for what this module grows into.
#[allow(unused_imports)]
use crate::prelude::*;

/// The label carrying the session id, on every session container.
///
/// Recovery lists containers by this key to find the sessions it owns
/// (`ARCHITECTURE.md`, "Engine adapter").
pub const LABEL_SESSION_ID: &str = "mars.session_id";

/// The label carrying the project id.
pub const LABEL_PROJECT_ID: &str = "mars.project_id";

/// The label carrying the agent profile id.
pub const LABEL_PROFILE_ID: &str = "mars.profile_id";

/// The container name for a session: `mars-session-<sid>`
/// (`ARCHITECTURE.md`, "Engine adapter").
///
/// The name is derived, never stored: recovery can find a session's container
/// from the session row alone, and the engine's own uniqueness check on names
/// is what stops two launches of the same session running side by side.
pub fn session_container_name(sid: Uuid) -> String {
    format!("mars-session-{sid}")
}

/// Which engine is behind the Docker-compatible socket, detected once from
/// `/version` when the adapter connects (ADR 0004).
///
/// It decides exactly one thing in v1: whether `UsernsMode: keep-id` is set,
/// which Podman requires and Docker ignores (`ARCHITECTURE.md`, "Session
/// container specification", Uid contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    /// Podman, through its Docker-compatible API.
    Podman,
    /// Docker.
    Docker,
}

impl fmt::Display for EngineKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineKind::Podman => f.write_str("podman"),
            EngineKind::Docker => f.write_str("docker"),
        }
    }
}

/// A container id as the engine returns it.
///
/// A newtype rather than a `String` so a container id cannot be passed where a
/// session id, a name or an image reference is expected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContainerId(pub String);

impl fmt::Display for ContainerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One bind mount: a host path made visible inside the container.
///
/// The source is a *host* path — `DATA_DIR_HOST`, not `DATA_DIR` — because the
/// engine resolves it on the host, which is not the orchestrator's filesystem
/// when the orchestrator runs in a container (`ARCHITECTURE.md`, "Storage").
/// Nested binds are legal and the launcher orders parents before children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    /// The path on the host, as the engine will resolve it.
    pub host_source: PathBuf,
    /// The absolute path inside the container.
    pub container_target: String,
    /// Whether the mount is read-only.
    pub read_only: bool,
}

/// Everything the adapter needs to create one container.
///
/// The field set is `ARCHITECTURE.md`, "Session container specification",
/// minus the values that are the same for every session and are therefore the
/// adapter's to set.
///
/// [`Debug`] is implemented by hand: it prints the *keys* of [`env`](Self::env)
/// and never the values, so a spec that has had a project's secrets resolved
/// into it cannot leak them through a `{:?}` in a log line or an error
/// (CLAUDE.md rule 3).
#[derive(Clone, PartialEq, Eq)]
pub struct ContainerSpec {
    /// The image reference to run, pulled at launch if absent.
    pub image: String,
    /// The container name; [`session_container_name`] for a session.
    pub name: String,
    /// The labels to set, at least [`LABEL_SESSION_ID`], [`LABEL_PROJECT_ID`]
    /// and [`LABEL_PROFILE_ID`] for a session.
    pub labels: BTreeMap<String, String>,
    /// The user to run as, `1000:1000` for a session (Uid contract).
    pub user: String,
    /// The working directory, `/session/work` for a session.
    pub working_dir: String,
    /// The command; empty leaves the image's own.
    pub cmd: Vec<String>,
    /// The environment, in order.
    ///
    /// An ordered `Vec` and not a map, because the order is part of the
    /// contract: the fixed variables first, the resolved secrets after, so a
    /// project secret cannot shadow `MARS_SESSION_ID` or `HOME`. Duplicates
    /// are therefore possible and the last one wins, which is exactly what
    /// that ordering relies on.
    pub env: Vec<(String, String)>,
    /// The bind mounts, parents before children.
    pub binds: Vec<Bind>,
    /// The network the container is created on — the `NetworkMode`, so it is
    /// attached before the container ever runs. The egress network is a second
    /// network the caller connects with
    /// [`connect_network`](super::ContainerEngine::connect_network) before
    /// [`start`](super::ContainerEngine::start).
    pub network: String,
    /// `ExtraHosts` entries, `name:address` each, from `SESSION_EXTRA_HOSTS`.
    pub extra_hosts: Vec<String>,
    /// The `HostConfig.Runtime` to use, from the profile; `None` leaves the
    /// engine's default.
    pub runtime: Option<String>,
    /// Whether stdin is kept open and attachable (`OpenStdin`). The session
    /// owner writes the CLI's input through it.
    pub open_stdin: bool,
}

impl fmt::Debug for ContainerSpec {
    /// Redacted: every field except the environment *values*, which are the
    /// resolved secrets (rule 3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env_keys: Vec<&str> = self.env.iter().map(|(key, _)| key.as_str()).collect();

        f.debug_struct("ContainerSpec")
            .field("image", &self.image)
            .field("name", &self.name)
            .field("labels", &self.labels)
            .field("user", &self.user)
            .field("working_dir", &self.working_dir)
            .field("cmd", &self.cmd)
            .field("env_keys", &env_keys)
            .field("binds", &self.binds)
            .field("network", &self.network)
            .field("extra_hosts", &self.extra_hosts)
            .field("runtime", &self.runtime)
            .field("open_stdin", &self.open_stdin)
            .finish()
    }
}

/// The signal a `kill` sends.
///
/// [`Display`](fmt::Display) yields the upper-case name the engine's kill
/// endpoint accepts, and the same string the `state_change` event's `signal`
/// field carries (`SPEC.md`, "WebSocket: session stream"; `ARCHITECTURE.md`,
/// "Stop semantics").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    /// A stop request: ends the CLI's current turn cleanly and lets it write
    /// its `result`.
    Sigint,
    /// The hard stop after `STOP_GRACE_SECS`: the CLI exits 143 with the turn
    /// unfinished.
    Sigterm,
    /// Uninterruptible; not part of the stop sequence.
    Sigkill,
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Signal::Sigint => "SIGINT",
            Signal::Sigterm => "SIGTERM",
            Signal::Sigkill => "SIGKILL",
        })
    }
}

/// How a container ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    /// The process exit code. 130 for a SIGINT-stopped CLI, 143 for a
    /// SIGTERM-ed one (`ARCHITECTURE.md`, "Stop semantics").
    pub code: i64,
    /// Whether the kernel's OOM killer ended it, which the session owner
    /// reports rather than reading as an ordinary non-zero exit.
    pub oom_killed: bool,
}

/// The engine's view of a container's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    /// Created, never started.
    Created,
    /// Running now.
    Running,
    /// Paused. Mars never pauses a container; recovery may still find one.
    Paused,
    /// Exited, with the code it exited on.
    Exited {
        /// The process exit code.
        code: i64,
    },
    /// Removal is in progress.
    Removing,
    /// The engine could not clean it up.
    Dead,
    /// A state string this version does not know, kept verbatim so a log line
    /// can say what the engine actually reported.
    Unknown(String),
}

impl ContainerState {
    /// Whether the container is running right now. Recovery adopts a session
    /// whose container is running and parks one whose container is not.
    pub fn is_running(&self) -> bool {
        matches!(self, ContainerState::Running)
    }
}

/// The full answer to an inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    /// The engine's id for the container.
    pub id: ContainerId,
    /// The container's name, without the engine's leading slash.
    pub name: String,
    /// Every label the container carries.
    pub labels: BTreeMap<String, String>,
    /// The state the engine reports.
    pub state: ContainerState,
    /// The names of the networks the container is attached to. Recovery checks
    /// that a readopted container is still on both.
    pub networks: Vec<String>,
    /// The host pid of the container's main process, when the engine reports
    /// one and the container is running.
    pub pid: Option<i64>,
}

/// One row of a list, which carries less than an inspect does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSummary {
    /// The engine's id for the container.
    pub id: ContainerId,
    /// The container's name, without the engine's leading slash.
    pub name: String,
    /// Every label the container carries; recovery reads
    /// [`LABEL_SESSION_ID`] out of it.
    pub labels: BTreeMap<String, String>,
    /// Whether the engine reports it running.
    pub running: bool,
}

/// The write half of an attached container's stdin.
///
/// A trait object rather than a concrete type so the bollard implementation
/// can hand back whatever its attach returns and the mock can hand back a
/// buffer. The blanket implementation means any `AsyncWrite` qualifies: there
/// is nothing to implement, only to box.
pub trait StdinWriter: tokio::io::AsyncWrite + Send + Unpin {}

impl<T: tokio::io::AsyncWrite + Send + Unpin> StdinWriter for T {}

/// A live `exec` with a PTY: the optional terminal view.
///
/// `SPEC.md`, "WebSocket: session stream": the terminal is an exec running
/// `/bin/bash -l` as `agent`, its output goes to the client in binary frames,
/// `terminal_resize` calls [`resize`](Self::resize), and the exit code
/// [`close`](Self::close) returns is the `exit_code` of `terminal_closed`.
/// Nothing it carries is recorded as an event.
#[async_trait]
pub trait ExecSession: Send {
    /// The next chunk of PTY output, or `None` once the PTY is at end of
    /// stream. A PTY multiplexes stdout and stderr, so there is one stream.
    async fn read(&mut self) -> Result<Option<Bytes>, EngineError>;

    /// Write bytes to the PTY.
    async fn write(&mut self, data: &[u8]) -> Result<(), EngineError>;

    /// Resize the PTY, in character cells.
    async fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError>;

    /// Drop the input half, let the shell exit, and answer with its exit code.
    ///
    /// Takes `Box<Self>` because that is what the caller holds and because
    /// consuming the session is the point: closing twice is not a thing that
    /// can be expressed.
    async fn close(self: Box<Self>) -> Result<i64, EngineError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_display_is_the_upper_case_name_the_engine_and_the_event_use() {
        assert_eq!(Signal::Sigint.to_string(), "SIGINT");
        assert_eq!(Signal::Sigterm.to_string(), "SIGTERM");
        assert_eq!(Signal::Sigkill.to_string(), "SIGKILL");
    }

    #[test]
    fn a_session_container_is_named_after_its_session() {
        let sid = Uuid::parse_str("00000000-0000-4000-8000-00000000abcd").expect("a valid uuid");
        assert_eq!(
            session_container_name(sid),
            "mars-session-00000000-0000-4000-8000-00000000abcd"
        );
    }

    #[test]
    fn a_container_id_displays_as_its_string() {
        assert_eq!(ContainerId("deadbeef".to_string()).to_string(), "deadbeef");
    }

    /// A spec with an obviously fake secret in it (rule 3).
    fn spec_with_a_secret() -> ContainerSpec {
        ContainerSpec {
            image: "mars-session-claude:dev".to_string(),
            name: "mars-session-00000000-0000-4000-8000-00000000abcd".to_string(),
            labels: BTreeMap::from([(
                LABEL_SESSION_ID.to_string(),
                "00000000-0000-4000-8000-00000000abcd".to_string(),
            )]),
            user: "1000:1000".to_string(),
            working_dir: "/session/work".to_string(),
            cmd: vec!["claude".to_string()],
            env: vec![
                ("HOME".to_string(), "/session/home".to_string()),
                (
                    "ANTHROPIC_API_KEY".to_string(),
                    "not-a-real-api-key".to_string(),
                ),
            ],
            binds: vec![Bind {
                host_source: PathBuf::from("/srv/mars/data/sessions/s/work"),
                container_target: "/session/work".to_string(),
                read_only: false,
            }],
            network: "mars-sessions".to_string(),
            extra_hosts: vec!["host.containers.internal:host-gateway".to_string()],
            runtime: None,
            open_stdin: true,
        }
    }

    #[test]
    fn a_spec_debugs_its_env_keys_and_never_its_env_values() {
        let debug = format!("{:?}", spec_with_a_secret());

        assert!(debug.contains("HOME"), "missing env key: {debug}");
        assert!(
            debug.contains("ANTHROPIC_API_KEY"),
            "missing env key: {debug}"
        );
        assert!(
            !debug.contains("not-a-real-api-key"),
            "leaked an env value: {debug}"
        );
        assert!(
            !debug.contains("/session/home"),
            "leaked an env value: {debug}"
        );
        // The rest of the spec is still readable, which is what makes the
        // redacted debug worth having at all.
        assert!(debug.contains("mars-session-claude:dev"), "lost: {debug}");
        assert!(debug.contains("/session/work"), "lost: {debug}");
    }

    #[test]
    fn a_spec_and_a_bind_compare_by_value_so_a_mock_can_record_and_a_test_assert() {
        assert_eq!(spec_with_a_secret(), spec_with_a_secret().clone());

        let mut other = spec_with_a_secret();
        other
            .env
            .push(("MARS_TASK_ID".to_string(), "t1".to_string()));
        assert_ne!(spec_with_a_secret(), other);
    }

    #[test]
    fn only_running_is_running() {
        assert!(ContainerState::Running.is_running());
        for state in [
            ContainerState::Created,
            ContainerState::Paused,
            ContainerState::Exited { code: 0 },
            ContainerState::Removing,
            ContainerState::Dead,
            ContainerState::Unknown("restarting".to_string()),
        ] {
            assert!(!state.is_running(), "unexpectedly running: {state:?}");
        }
    }

    #[test]
    fn an_engine_kind_displays_in_lower_case_for_a_log_field() {
        assert_eq!(EngineKind::Podman.to_string(), "podman");
        assert_eq!(EngineKind::Docker.to_string(), "docker");
    }
}
