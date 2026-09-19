//! The production [`ContainerEngine`](super::ContainerEngine) implementation,
//! on `bollard` against the Docker-compatible API (ADR 0004).
//!
//! It is the only module in the crate that may name a `bollard` type:
//! everything else speaks the plain types in [`super::types`], and every
//! `bollard::errors::Error` becomes an [`EngineError`] through the single
//! [`From`] implementation below.
//!
//! It covers the adapter's foundations — connecting to the socket named by
//! `DOCKER_HOST`, deciding once whether the engine is Podman or Docker,
//! answering the health endpoint's ping and creating the two session networks
//! (`ARCHITECTURE.md`, "Networks") — and the container lifecycle the launcher,
//! the stop sequence, recovery and orphan cleanup run on: create, connect to
//! the egress network, start, stop, kill with a named signal, remove, inspect,
//! wait, list by label and pull an absent image. It also covers the two
//! streaming operations — attaching a container's stdin with the TTY off, and
//! running an exec with a PTY for the terminal view — whose halves live in
//! [`super::streams`]. Those are exactly the rows of the operation table in
//! `ARCHITECTURE.md`, "Engine adapter", and nothing outside it is called.
//!
//! Every method here is one engine call plus a mapping, with one exception:
//! `create` takes a per-engine-host mutex first, because Podman cannot resolve
//! `keep-id` for two containers at once (`ARCHITECTURE.md`, "Engine adapter",
//! the `UsernsMode` row). Retries, the grace
//! period between `SIGINT` and `SIGTERM` and the decision of what an exit code
//! means belong to the session owner (`ARCHITECTURE.md`, "Stop semantics").
//!
//! Note that this module shadows the `bollard` crate name inside [`super`].
//! This module and its siblings still reach the crate as `bollard::…`, because
//! a `use` path resolves through the extern prelude, but `engine/mod.rs`
//! itself would have to write `::bollard::…`.
//!
//! **Secrets.** Nothing here builds a message from a [`ContainerSpec`]'s
//! environment, and the engine socket's path never leaves this module in an
//! error: it appears once in the startup log line and nowhere else, so a
//! failure that reaches an HTTP caller cannot describe the host's filesystem
//! (CLAUDE.md rule 3).

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
// Shadows the prelude's one-parameter `Result<T>` alias, exactly as
// `engine/mod.rs` does, so the signatures below read as the trait declares
// them: `Result<T, EngineError>`.
use std::result::Result;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use async_trait::async_trait;
// `bollard` names three types the engine's own vocabulary also names. They are
// imported under an alias rather than qualified at every use, so a signature
// below can never be read as the wrong one.
use bollard::container::AttachContainerResults;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{
    ContainerInspectResponse, ContainerState as BollardContainerState,
    ContainerSummary as BollardContainerSummary, ContainerSummaryStateEnum, CreateImageInfo,
    NetworkConnectRequest, NetworkCreateRequest, SystemVersion,
};
use bollard::query_parameters::{
    AttachContainerOptionsBuilder, CreateContainerOptionsBuilder, CreateImageOptionsBuilder,
    InspectContainerOptions, InspectNetworkOptions, KillContainerOptionsBuilder,
    ListContainersOptionsBuilder, RemoveContainerOptionsBuilder, StartContainerOptions,
    StopContainerOptionsBuilder, WaitContainerOptionsBuilder,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::StreamExt;
use tokio::sync::Mutex;

use super::spec::to_bollard;
use super::streams::{BollardExec, BollardStdin, resize_exec};
use super::{
    ContainerEngine, ContainerId, ContainerInfo, ContainerSpec, ContainerState, ContainerSummary,
    EngineError, EngineKind, ExecSession, ExitStatus, Signal, StdinWriter,
};
use crate::prelude::*;

/// How long a single request to the engine may take, in seconds.
///
/// Generous on purpose: an image pull and a `wait` on a long-running session
/// both go through this client, and a timeout that is shorter than a pull
/// turns a slow registry into a failed launch.
const REQUEST_TIMEOUT_SECS: u64 = 120;

/// The driver both session networks are created with (`ARCHITECTURE.md`,
/// "Networks"). The only driver in the compatible subset of ADR 0004.
const NETWORK_DRIVER: &str = "bridge";

/// The `wait` condition: the engine answers once the container is no longer
/// running, which is what the session owner is waiting for. Both Docker and
/// Podman's compatible API accept it.
const WAIT_CONDITION: &str = "not-running";

/// The filter key a label list is built on. `label=<key>` matches every
/// container carrying the key, whatever its value, which is how recovery finds
/// every session container (`ARCHITECTURE.md`, "Engine adapter").
const LABEL_FILTER: &str = "label";

/// The `bollard` adapter: one connected client, plus what `/version` said
/// about the engine behind it when the connection was made.
///
/// The kind is read once and kept, rather than asked for per operation: it
/// decides `UsernsMode`, which every container creation needs and which cannot
/// change under a running orchestrator without the socket changing too
/// (ADR 0004).
#[derive(Debug, Clone)]
pub struct BollardEngine {
    /// The connected client. `Docker` is itself a cheap handle around a shared
    /// transport, so cloning the adapter does not open a second connection.
    docker: Docker,
    /// Podman or Docker, decided once by [`engine_kind_of`].
    kind: EngineKind,
    /// The engine's own version string, for the startup log and diagnostics.
    version: String,
    /// Held across [`ContainerEngine::create`] and released before anything
    /// else, so at most one container is being created on this engine host at a
    /// time (`ARCHITECTURE.md`, "Engine adapter", the `UsernsMode` row).
    ///
    /// Podman resolves `keep-id` by calling the non-thread-safe
    /// `subid_get_uid_ranges` in `libsubid` inside the API service process,
    /// once per create: two creates in flight corrupt each other's uid mapping
    /// — no sub-uid range at all, or the range counted twice — and the
    /// container then fails at `start`, or the service segfaults and fails
    /// every request it is serving (Bears u6zkz). The mapping is already wrong
    /// when `create` returns, so serialising `create` alone is enough.
    ///
    /// Taken whatever the engine is rather than only on
    /// [`EngineKind::Podman`]: a create is a handful of milliseconds against a
    /// local socket and sessions are not launched in bursts, so the branch
    /// would buy Docker nothing measurable and cost the adapter a second code
    /// path that only one engine ever exercises.
    ///
    /// Shared per engine host by [`create_lock_for`] rather than made per
    /// adapter: a clone of the adapter must take the same lock, or there is no
    /// lock, and so must a second adapter connected to the same socket — the
    /// corruption is in the engine's own process, so what has to be serialised
    /// is the host, not the client handle. `main` connects once, but the engine
    /// suite connects per scenario and runs them in parallel.
    create_lock: Arc<Mutex<()>>,
}

impl BollardEngine {
    /// Connect to the engine socket `docker_host` names and ask it what it is.
    ///
    /// `unix://<path>` connects to a socket and `tcp://` or `http://` to a
    /// TCP endpoint (`README.md`, "Configuration"); anything else is refused
    /// by scheme before a connection is attempted. `/version` is called
    /// immediately, so an engine that is not running fails the orchestrator's
    /// startup rather than the first session launch.
    pub async fn connect(docker_host: &str) -> Result<Self, EngineError> {
        let docker = connect_client(docker_host)?;
        let (docker, version) = describe(docker).await?;

        let kind = engine_kind_of(&version);
        let version_string = version.version.unwrap_or_else(|| "unknown".to_string());

        // The one place the socket path is written down: a startup line the
        // operator needs, never an error a caller sees (rule 3).
        info!(
            engine_kind = %kind,
            engine_version = %version_string,
            docker_host = %docker_host,
            "connected to the container engine"
        );

        Ok(Self {
            docker,
            kind,
            version: version_string,
            create_lock: create_lock_for(docker_host),
        })
    }

    /// The engine's own version string, as `/version` reported it.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// One inspect, still in `bollard`'s own shape.
    ///
    /// [`ContainerEngine::inspect`] maps it to a [`ContainerInfo`], which does
    /// not carry the OOM flag; [`ContainerEngine::wait`] needs that flag, so
    /// both go through this rather than one through the other.
    async fn inspect_raw(&self, id: &ContainerId) -> Result<ContainerInspectResponse, EngineError> {
        // A 404 becomes `NotFound` through the single conversion, which is
        // what recovery reads as "the container is gone".
        Ok(self
            .docker
            .inspect_container(&id.0, None::<InspectContainerOptions>)
            .await?)
    }

    /// Create the network, treating a concurrent creation as success.
    ///
    /// Two orchestrators starting at once both see a 404 and both post to
    /// `/networks/create`; the engine settles it by answering one of them 409,
    /// which means the network now exists, which is all the caller asked for.
    async fn create_network(&self, name: &str, internal: bool) -> Result<(), EngineError> {
        let request = NetworkCreateRequest {
            name: name.to_string(),
            driver: Some(NETWORK_DRIVER.to_string()),
            internal: Some(internal),
            // `ipam`, `enable_ipv6` and `options` stay unset: they are outside
            // the compatible subset verified on both engines (ADR 0004).
            ..Default::default()
        };

        match self.docker.create_network(request).await {
            Ok(_) => {
                info!(network = %name, internal, "created the session network");
                Ok(())
            }
            Err(error) => match EngineError::from(error) {
                EngineError::Conflict(_) => {
                    debug!(network = %name, "the session network was created concurrently");
                    Ok(())
                }
                other => Err(other),
            },
        }
    }
}

/// The create lock for one engine host, created the first time that host is
/// connected to and shared by every adapter connected to it afterwards.
///
/// Keyed by the trimmed `DOCKER_HOST` value, which is how the engine is named
/// everywhere else in the process; two spellings of the same socket would get a
/// lock each, and nothing in Mars spells it two ways. The registry is a plain
/// `std` mutex because it is held only long enough to clone an `Arc` — never
/// across an await.
fn create_lock_for(docker_host: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<StdMutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

    LOCKS
        .get_or_init(Default::default)
        .lock()
        .expect("the engine create-lock registry is not poisoned")
        .entry(docker_host.trim().to_string())
        .or_default()
        .clone()
}

/// Build the client for `docker_host`, without talking to it.
///
/// Split out from [`BollardEngine::connect`] because it is the whole of the
/// scheme rule and can therefore be tested without an engine.
fn connect_client(docker_host: &str) -> Result<Docker, EngineError> {
    let host = docker_host.trim();

    let client = if let Some(path) = host.strip_prefix("unix://") {
        Docker::connect_with_socket(path, REQUEST_TIMEOUT_SECS, API_DEFAULT_VERSION)
    } else if host.starts_with("tcp://") || host.starts_with("http://") {
        Docker::connect_with_http(host, REQUEST_TIMEOUT_SECS, API_DEFAULT_VERSION)
    } else {
        return Err(EngineError::Connection(format!(
            "DOCKER_HOST has the unsupported scheme {}; expected unix://, tcp:// or http://",
            scheme_of(host)
        )));
    };

    client.map_err(|error| match error {
        // bollard puts the path into this message; the adapter does not
        // (rule 3, and the edge case in the task: the path is a startup-log
        // detail, not something an HTTP caller is told).
        bollard::errors::Error::SocketNotFoundError(_) => EngineError::Connection(
            "the engine socket named by DOCKER_HOST does not exist".to_string(),
        ),
        other => EngineError::from(other),
    })
}

/// The scheme of a `DOCKER_HOST` value, for the message that rejects it.
///
/// Only the scheme, never the rest: an address can carry a host name or a
/// socket path, and neither belongs in an error.
fn scheme_of(host: &str) -> &str {
    match host.split_once("://") {
        Some((scheme, _)) if !scheme.is_empty() => scheme,
        _ => "(none)",
    }
}

/// Ask the engine what it is, renegotiating the API version once if it refuses
/// the client's.
///
/// Podman's compatible API has answered with a lower maximum API version than
/// bollard's default in the past; `negotiate_version` re-reads `/version` and
/// steps the client down to what the engine supports. It consumes the client,
/// which is why this takes and returns one.
async fn describe(docker: Docker) -> Result<(Docker, SystemVersion), EngineError> {
    match docker.version().await {
        Ok(version) => Ok((docker, version)),
        Err(error) if is_api_version_mismatch(&error) => {
            warn!(
                engine_error = %error,
                "the engine refused the client API version; renegotiating"
            );

            let docker = docker.negotiate_version().await?;
            let version = docker.version().await?;

            info!(
                api_version = %version.api_version.as_deref().unwrap_or("unknown"),
                "renegotiated the container engine API version"
            );

            Ok((docker, version))
        }
        Err(error) => Err(error.into()),
    }
}

/// Whether the engine refused the request because the client's API version is
/// newer than the engine supports.
///
/// The engine answers 400 and says so in words; there is no distinct status
/// for it, so the message is what there is to read.
fn is_api_version_mismatch(error: &bollard::errors::Error) -> bool {
    match error {
        bollard::errors::Error::DockerResponseServerError {
            status_code,
            message,
        } => {
            let message = message.to_lowercase();
            *status_code == 400
                && (message.contains("client version") || message.contains("api version"))
        }
        _ => false,
    }
}

/// Podman or Docker, from what `/version` reported (ADR 0004).
///
/// Podman's compatible API names itself in a component (`Podman Engine`) and
/// in `Platform.Name` (`podman`); Docker names neither. Anything that claims
/// neither is treated as Docker, which is the conservative answer: the only
/// behavioural consequence in v1 is `UsernsMode: keep-id`, which Podman
/// requires and Docker ignores, and the startup probe fails loudly if an
/// engine that needed it did not get it (`ARCHITECTURE.md`, "Engine adapter").
fn engine_kind_of(version: &SystemVersion) -> EngineKind {
    let named_in_a_component = version
        .components
        .iter()
        .flatten()
        .any(|component| names_podman(&component.name));

    let named_in_the_platform = version
        .platform
        .as_ref()
        .is_some_and(|platform| names_podman(&platform.name));

    if named_in_a_component || named_in_the_platform {
        EngineKind::Podman
    } else {
        EngineKind::Docker
    }
}

/// Whether a `/version` name claims Podman, ignoring case.
fn names_podman(name: &str) -> bool {
    name.to_lowercase().contains("podman")
}

/// Whether an engine's refusal says the thing already exists.
///
/// The one message the adapter reads rather than only reports: connecting a
/// container to a network it is already on is success, and the status alone
/// does not say which refusal it was (Docker answers 403, Podman 409, and both
/// use those statuses for other things too).
fn says_already(message: &str) -> bool {
    message.to_lowercase().contains("already")
}

/// Whether an engine's refusal of an exec says the container is not running.
///
/// The second message the adapter reads rather than only reports, and for the
/// same reason as [`says_already`]: the status alone does not say which
/// refusal it was. Docker answers 409 — already a [`EngineError::Conflict`] —
/// but a rootless Podman 6 answers 500 with `can only create exec sessions on
/// running containers: container state improper`, and a 500 would reach the
/// WebSocket handler as an internal fault rather than as the `error` answer to
/// a `terminal_open` that is simply too late (`SPEC.md`, "WebSocket: session
/// stream").
fn says_not_running(message: &str) -> bool {
    let message = message.to_lowercase();

    message.contains("not running")
        || message.contains("state improper")
        || message.contains("running containers")
}

/// Whether an engine's refusal of a removal says the container is still
/// running and would need `force`.
///
/// The third message the adapter reads rather than only reports, for the reason
/// [`says_already`] gives. Docker refuses with 409 — already an
/// [`EngineError::Conflict`] — but a rootless Podman 6 answers 500 with
/// `cannot remove container <id> as it is running - running or paused
/// containers cannot be removed without force: container state improper`, and a
/// 500 would reach the caller as an internal fault rather than as the state
/// conflict it is (`ARCHITECTURE.md`, "Engine adapter", Normalised semantics).
/// The two spellings of `force` are what is matched — Podman's `without force`
/// and Docker's `force remove` — because that is the part of either refusal
/// that says which conflict it was, and neither can be read as its opposite the
/// way a bare `is running` could. Docker's own wording is matched although its
/// 409 never reaches here, so an engine that answers it with another status is
/// still normalised.
fn says_needs_force(message: &str) -> bool {
    let message = message.to_lowercase();

    message.contains("without force") || message.contains("force remove")
}

/// How many times a listing that could not be decoded is asked for again, and
/// how long between two attempts: two seconds in all, far longer than a
/// container spends between `running` and `exited`.
const LIST_RETRIES: u32 = 20;
const LIST_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// Whether an engine's refusal of a create says the container name is taken.
///
/// The fourth message the adapter reads rather than only reports, for the
/// reason [`says_already`] gives. Docker and Podman 6 refuse with 409 — already
/// an [`EngineError::Conflict`] — but the Podman 4 series answers 500 with
/// `creating container storage: the container name "<name>" is already in use
/// by <id>. You have to remove that container to be able to reuse that name`
/// (seen on the Engine workflow's runner), and a 500 would reach the launcher
/// as an internal fault rather than as the conflict it is (`ARCHITECTURE.md`,
/// "Engine adapter", Normalised semantics). Docker's wording — `The container
/// name "/<name>" is already in use by container "<id>"` — shares the phrase
/// that is matched, so an engine that numbers it differently is still
/// normalised.
fn says_name_in_use(message: &str) -> bool {
    let message = message.to_lowercase();

    message.contains("container name") && message.contains("already in use")
}

/// The failure a pull stream reported inside an otherwise successful response,
/// if it reported one.
fn pull_error_of(info: &CreateImageInfo) -> Option<String> {
    let detail = info.error_detail.as_ref()?;

    Some(match (&detail.message, detail.code) {
        (Some(message), _) => message.clone(),
        (None, Some(code)) => format!("the registry answered {code}"),
        (None, None) => "the pull failed without a message".to_string(),
    })
}

/// The engine's state string and exit code as a [`ContainerState`].
///
/// `restarting`, the engine's empty string and anything a later API version
/// adds arrive as [`ContainerState::Unknown`] with the string kept verbatim,
/// so a log line can say what the engine actually reported.
fn container_state_of(status: &str, exit_code: i64) -> ContainerState {
    match status {
        "created" => ContainerState::Created,
        "running" => ContainerState::Running,
        "paused" => ContainerState::Paused,
        "exited" => ContainerState::Exited { code: exit_code },
        "removing" => ContainerState::Removing,
        "dead" => ContainerState::Dead,
        other => ContainerState::Unknown(other.to_string()),
    }
}

/// The exit code of a state that is an exit, for the wait that had to ask.
fn exited_code(state: Option<&BollardContainerState>) -> Option<i64> {
    let state = state?;

    match container_state_of(&status_string(state), state.exit_code.unwrap_or_default()) {
        ContainerState::Exited { code } => Some(code),
        _ => None,
    }
}

/// The engine's state string, which the enum's [`Display`](fmt::Display)
/// yields in the lower case the API sends.
fn status_string(state: &BollardContainerState) -> String {
    state
        .status
        .map(|status| status.to_string())
        .unwrap_or_default()
}

/// The engine's name for a container, without the leading slash it prefixes
/// for historic reasons.
fn strip_leading_slash(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

/// The engine's labels as the ordered map the plain types use.
fn labels_of(labels: Option<HashMap<String, String>>) -> BTreeMap<String, String> {
    labels.unwrap_or_default().into_iter().collect()
}

/// One inspect response as a [`ContainerInfo`].
///
/// The networks come back sorted rather than in the engine's hash order, so
/// two inspects of the same container compare equal and a log line reads the
/// same twice.
fn to_container_info(response: ContainerInspectResponse) -> ContainerInfo {
    let state = response.state.as_ref();
    let status = state.map(status_string).unwrap_or_default();
    let exit_code = state.and_then(|state| state.exit_code).unwrap_or_default();

    let mut networks: Vec<String> = response
        .network_settings
        .and_then(|settings| settings.networks)
        .unwrap_or_default()
        .into_keys()
        .collect();
    networks.sort();

    ContainerInfo {
        id: ContainerId(response.id.unwrap_or_default()),
        name: response
            .name
            .as_deref()
            .map(strip_leading_slash)
            .unwrap_or_default()
            .to_string(),
        labels: labels_of(response.config.and_then(|config| config.labels)),
        state: container_state_of(&status, exit_code),
        networks,
        // The engine reports 0 for a container that is not running, which is
        // not a pid; `None` is what the field means there.
        pid: state.and_then(|state| state.pid).filter(|pid| *pid > 0),
    }
}

/// One list row as a [`ContainerSummary`].
fn to_container_summary(summary: BollardContainerSummary) -> ContainerSummary {
    ContainerSummary {
        id: ContainerId(summary.id.unwrap_or_default()),
        name: summary
            .names
            .as_deref()
            .and_then(|names| names.first())
            .map(|name| strip_leading_slash(name).to_string())
            .unwrap_or_default(),
        running: summary.state == Some(ContainerSummaryStateEnum::RUNNING),
        labels: labels_of(summary.labels),
    }
}

/// The single translation from a `bollard` failure into an [`EngineError`].
///
/// It lives here because this is the only module that sees a `bollard` type.
/// The distinctions are the ones callers branch on: a missing object, a state
/// conflict, any other answer the engine gave, and everything that never got
/// an answer at all.
impl From<bollard::errors::Error> for EngineError {
    fn from(error: bollard::errors::Error) -> Self {
        match error {
            bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                message,
            } => EngineError::NotFound(message),
            bollard::errors::Error::DockerResponseServerError {
                status_code: 409,
                message,
            } => EngineError::Conflict(message),
            bollard::errors::Error::DockerResponseServerError {
                status_code,
                message,
            } => EngineError::Api {
                status: status_code,
                message,
            },
            // Everything else — transport, I/O, hyper, a body that would not
            // parse — is the engine failing to answer, which is what
            // `Connection` says.
            other => EngineError::Connection(other.to_string()),
        }
    }
}

#[async_trait]
impl ContainerEngine for BollardEngine {
    fn kind(&self) -> EngineKind {
        self.kind
    }

    async fn ping(&self) -> Result<(), EngineError> {
        // Every failure is the same answer here: the health endpoint reports
        // one bool (`SPEC.md`, "Health"), and an engine that cannot answer its
        // own ping is unreachable whatever it said.
        self.docker
            .ping()
            .await
            .map(|_| ())
            .map_err(|error| EngineError::Connection(error.to_string()))
    }

    async fn ensure_network(&self, name: &str, internal: bool) -> Result<(), EngineError> {
        match self
            .docker
            .inspect_network(name, None::<InspectNetworkOptions>)
            .await
        {
            Ok(existing) => {
                // An existing network is left exactly as it is. Recreating one
                // would disconnect whatever is attached to it, and a network
                // in use is not this component's to take down
                // (`ARCHITECTURE.md`, "Networks"); the mismatch is worth a
                // line in the log, and nothing more.
                if existing.internal.unwrap_or(false) != internal {
                    warn!(
                        network = %name,
                        expected_internal = internal,
                        "the network exists with a different internal flag; leaving it alone"
                    );
                }

                if let Some(driver) = existing.driver.as_deref()
                    && driver != NETWORK_DRIVER
                {
                    warn!(
                        network = %name,
                        driver = %driver,
                        "the network exists with a different driver; leaving it alone"
                    );
                }

                Ok(())
            }
            Err(error) => match EngineError::from(error) {
                EngineError::NotFound(_) => self.create_network(name, internal).await,
                other => Err(other),
            },
        }
    }

    async fn image_exists(&self, image: &str) -> Result<bool, EngineError> {
        match self.docker.inspect_image(image).await {
            Ok(_) => Ok(true),
            Err(error) => match EngineError::from(error) {
                // The only answer that means "not here". Every other failure
                // is propagated: an unreachable engine is not an absent image,
                // and answering `false` would send the launcher into a pull
                // that cannot work either.
                EngineError::NotFound(_) => Ok(false),
                other => Err(other),
            },
        }
    }

    async fn pull_image(&self, image: &str) -> Result<(), EngineError> {
        // The reference goes to the engine exactly as the profile wrote it. A
        // tag or a digest is the operator's choice, and defaulting `latest`
        // here would quietly pull something they did not ask for; the engine
        // applies its own default when there is no tag.
        let options = CreateImageOptionsBuilder::new().from_image(image).build();
        let mut stream = std::pin::pin!(self.docker.create_image(Some(options), None, None));

        while let Some(item) = stream.next().await {
            match item {
                Ok(info) => {
                    // A pull that fails part-way still answers 200 and reports
                    // the failure as an item in the stream, so an error item is
                    // a failed pull even though the request itself succeeded.
                    if let Some(message) = pull_error_of(&info) {
                        return Err(EngineError::ImagePull {
                            image: image.to_string(),
                            message,
                        });
                    }

                    if let Some(status) = info.status.as_deref() {
                        // `debug` and no higher: a pull reports a line per
                        // layer per percent.
                        debug!(image = %image, status = %status, "image pull progress");
                    }
                }
                Err(error) => {
                    return Err(EngineError::ImagePull {
                        image: image.to_string(),
                        // Verbatim: the launcher stores this in
                        // `sessions.error`, where a missing tag or an
                        // unauthenticated registry is the operator's answer.
                        message: error.to_string(),
                    });
                }
            }
        }

        info!(image = %image, "pulled the image");
        Ok(())
    }

    async fn create(&self, spec: &ContainerSpec) -> Result<ContainerId, EngineError> {
        let options = CreateContainerOptionsBuilder::new()
            .name(&spec.name)
            .build();

        // Every `HostConfig` field comes from the spec builder and none is
        // added here, so there is exactly one place where the container's
        // shape is decided (`ARCHITECTURE.md`, "Session container
        // specification").
        let body = to_bollard(spec, self.kind);

        // One create at a time on this engine host, whatever the engine: Podman
        // cannot resolve `keep-id` for two containers at once (see
        // [`Self::create_lock`]). The guard covers the create call and nothing
        // else — it is dropped when this `match` ends, before the caller's
        // `connect_network` and `start`, which are safe concurrently.
        let _creating = self.create_lock.lock().await;

        match self.docker.create_container(Some(options), body).await {
            Ok(response) => {
                for warning in &response.warnings {
                    warn!(
                        name = %spec.name,
                        warning = %warning,
                        "the engine warned about the container it created"
                    );
                }

                let id = ContainerId(response.id);
                // The id and the name, never the spec: its `Debug` redacts the
                // environment values, and this line has no reason to carry
                // them at all (rule 3).
                info!(container = %id, name = %spec.name, "created the container");
                Ok(id)
            }
            Err(error) => Err(match EngineError::from(error) {
                // The image went away between the launcher's `image_exists`
                // and this create. Naming it turns a bare "no such object"
                // into the one thing the operator can act on. Nothing pulls
                // from in here: a pull failure has to reach `sessions.error`
                // as itself, which is why the launcher pulls first.
                EngineError::NotFound(message) => {
                    EngineError::NotFound(format!("image {}: {message}", spec.image))
                }
                // A name already in use is a `Conflict`, which is how a second
                // launch of the same session is refused. Docker and Podman 6
                // number it 409; an older Podman numbers it 500 (see
                // `says_name_in_use`), and the caller sees the same `Conflict`
                // on either.
                EngineError::Api { status, message } if says_name_in_use(&message) => {
                    debug!(name = %spec.name, status, "the container name is already in use");
                    EngineError::Conflict(message)
                }
                other => other,
            }),
        }
    }

    async fn connect_network(&self, id: &ContainerId, network: &str) -> Result<(), EngineError> {
        let request = NetworkConnectRequest {
            container: id.0.clone(),
            // Mars pins no address on either network, so there is no endpoint
            // configuration to send and the engine's defaults apply.
            endpoint_config: None,
        };

        match self.docker.connect_network(network, request).await {
            Ok(()) => {
                debug!(container = %id, network = %network, "connected the container to the network");
                Ok(())
            }
            // Already connected is what the caller wanted. Docker says so with
            // 403 and Podman with 409, and only when the message says
            // "already": any other refusal at those statuses is a real one.
            Err(error) => match EngineError::from(error) {
                EngineError::Conflict(message) if says_already(&message) => {
                    debug!(container = %id, network = %network, "the container was already on the network");
                    Ok(())
                }
                EngineError::Api {
                    status: 403,
                    message,
                } if says_already(&message) => {
                    debug!(container = %id, network = %network, "the container was already on the network");
                    Ok(())
                }
                other => Err(other),
            },
        }
    }

    async fn start(&self, id: &ContainerId) -> Result<(), EngineError> {
        // A container that is already running answers 304 on both engines, and
        // this version of bollard reads a 304 as success, so `start` is
        // idempotent without an arm of its own (`ARCHITECTURE.md`, "Engine
        // adapter", Normalised semantics). A missing container is the 404 the
        // single conversion turns into `NotFound`.
        self.docker
            .start_container(&id.0, None::<StartContainerOptions>)
            .await?;

        info!(container = %id, "started the container");
        Ok(())
    }

    async fn stop(&self, id: &ContainerId, grace_secs: u32) -> Result<(), EngineError> {
        let options = StopContainerOptionsBuilder::new()
            // The engine's timeout is signed seconds; a grace period that does
            // not fit is a misconfiguration, and saturating is friendlier than
            // wrapping it into a negative timeout, which means "wait forever".
            .t(i32::try_from(grace_secs).unwrap_or(i32::MAX))
            .build();

        match self.docker.stop_container(&id.0, Some(options)).await {
            Ok(()) => {
                info!(container = %id, grace_secs, "stopped the container");
                Ok(())
            }
            Err(error) => match EngineError::from(error) {
                // The container had already exited. The engine reports that
                // with 304, which this version of bollard already reads as
                // success; the arm stays for the engine or version that
                // surfaces it, because "it is already stopped" is what the
                // caller asked for either way.
                EngineError::Api { status: 304, .. } => {
                    debug!(container = %id, "the container had already stopped");
                    Ok(())
                }
                other => Err(other),
            },
        }
    }

    async fn kill(&self, id: &ContainerId, signal: Signal) -> Result<(), EngineError> {
        // The name, not the number: `ARCHITECTURE.md`, "Stop semantics" sends
        // SIGINT and then SIGTERM by name, and the same string goes into the
        // `state_change` event.
        let options = KillContainerOptionsBuilder::new()
            .signal(&signal.to_string())
            .build();

        // A container that has already exited is the engine's 409, which
        // arrives as `Conflict` and which the session owner reads as "already
        // gone" rather than as a failure.
        self.docker.kill_container(&id.0, Some(options)).await?;

        info!(container = %id, signal = %signal, "signalled the container");
        Ok(())
    }

    async fn remove(&self, id: &ContainerId, force: bool) -> Result<(), EngineError> {
        // `v` and `link` stay at their defaults, both false: a session's state
        // lives in bind mounts under `DATA_DIR`, not in anonymous volumes, and
        // there are no links to remove.
        let options = RemoveContainerOptionsBuilder::new().force(force).build();

        match self.docker.remove_container(&id.0, Some(options)).await {
            Ok(()) => {
                info!(container = %id, force, "removed the container");
                Ok(())
            }
            Err(error) => match EngineError::from(error) {
                // A container that is not there is already removed, which is
                // what orphan cleanup and the end of a session both want.
                EngineError::NotFound(_) => {
                    debug!(container = %id, "the container was already gone");
                    Ok(())
                }
                // A running container refused without `force` is a state
                // conflict, and Podman numbers it 500 where Docker numbers it
                // 409 (see `says_needs_force`); the caller sees the same
                // `Conflict` on either engine.
                EngineError::Api { status, message } if says_needs_force(&message) => {
                    debug!(container = %id, status, "the container is still running");
                    Err(EngineError::Conflict(message))
                }
                other => Err(other),
            },
        }
    }

    async fn inspect(&self, id: &ContainerId) -> Result<ContainerInfo, EngineError> {
        Ok(to_container_info(self.inspect_raw(id).await?))
    }

    async fn wait(&self, id: &ContainerId) -> Result<ExitStatus, EngineError> {
        let options = WaitContainerOptionsBuilder::new()
            .condition(WAIT_CONDITION)
            .build();

        // A container that has already exited answers straight away, which is
        // what makes this safe to call on a container a restart readopted.
        let mut stream = std::pin::pin!(self.docker.wait_container(&id.0, Some(options)));

        let code = match stream.next().await {
            Some(Ok(response)) => Some(response.status_code),
            // bollard turns a non-zero exit into an error of its own. A
            // non-zero exit is not a failure here: 130 and 143 are exactly
            // what a stopped session exits with (`ARCHITECTURE.md`, "Stop
            // semantics").
            Some(Err(::bollard::errors::Error::DockerContainerWaitError { code, .. })) => {
                Some(code)
            }
            Some(Err(error)) => return Err(error.into()),
            // The engine ended the stream without answering; the inspect below
            // has the exit code, if the container did exit.
            None => None,
        };

        // One inspect answers the OOM flag, which the wait response does not
        // carry, and — when the stream said nothing — the exit code itself. A
        // container removed between the exit and this call still has an exit
        // code to report, so a failed inspect costs the flag and nothing more.
        let state = self.inspect_raw(id).await.ok().and_then(|info| info.state);
        let oom_killed = state
            .as_ref()
            .and_then(|state| state.oom_killed)
            .unwrap_or(false);

        let code = match code {
            Some(code) => code,
            None => exited_code(state.as_ref()).ok_or_else(|| {
                EngineError::Connection(format!(
                    "the engine ended the wait for container {id} without an exit status"
                ))
            })?,
        };

        debug!(container = %id, exit_code = code, oom_killed, "the container exited");
        Ok(ExitStatus { code, oom_killed })
    }

    async fn list_by_label(&self, label_key: &str) -> Result<Vec<ContainerSummary>, EngineError> {
        // `all`, because an exited session container is exactly what recovery
        // and orphan cleanup are looking for.
        let filters = HashMap::from([(LABEL_FILTER.to_string(), vec![label_key.to_string()])]);
        let options = ListContainersOptionsBuilder::new()
            .all(true)
            .filters(&filters)
            .build();

        // A listing can catch a container between two states the engine API
        // has no name for: the Podman 4 series reports `stopped` for a
        // container whose process has ended and whose clean-up has not
        // finished, and the typed response refuses the unknown variant, which
        // fails the whole listing and not only that row. The state is over in
        // milliseconds — the container becomes `exited` — so the listing is
        // asked for again rather than reported as an engine that is down:
        // startup recovery treats a failed listing as fatal
        // (`ARCHITECTURE.md`, "Restart procedure").
        let mut attempt = 0;
        let containers = loop {
            match self.docker.list_containers(Some(options.clone())).await {
                Ok(containers) => break containers,
                Err(bollard::errors::Error::JsonDataError { message, .. })
                    if attempt < LIST_RETRIES =>
                {
                    attempt += 1;
                    debug!(
                        attempt,
                        error = %message,
                        "the container listing held a state in transition; asking again"
                    );
                    tokio::time::sleep(LIST_RETRY_DELAY).await;
                }
                Err(err) => return Err(err.into()),
            }
        };

        Ok(containers.into_iter().map(to_container_summary).collect())
    }

    async fn attach_stdin(&self, id: &ContainerId) -> Result<Box<dyn StdinWriter>, EngineError> {
        // Stdin and nothing else, with `logs` off so the engine replays
        // nothing: the CLI's output is the transcript file the owner tails,
        // and this socket is a pipe for input (ADR 0010; `ARCHITECTURE.md`,
        // "Session container specification", Stdin row). `Tty` is off on the
        // container itself, which is what keeps the two directions apart.
        let options = AttachContainerOptionsBuilder::new()
            .stdin(true)
            .stdout(false)
            .stderr(false)
            .stream(true)
            .logs(false)
            .build();

        let AttachContainerResults { output, input } =
            self.docker.attach_container(&id.0, Some(options)).await?;

        info!(container = %id, "attached to the container's stdin");

        // The output half goes with the writer, drained by a task of its own:
        // a hijacked connection nobody reads is one the engine may close, and
        // the session owner never looks at output.
        Ok(Box::new(BollardStdin::attached(id.clone(), input, output)))
    }

    async fn exec_pty(
        &self,
        id: &ContainerId,
        cmd: &[String],
        user: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Box<dyn ExecSession>, EngineError> {
        // A container that is not running refuses the exec, and the refusal is
        // a `Conflict` however the engine numbered it (see `says_not_running`).
        // That is the one the WebSocket handler answers a `terminal_open` with
        // an `error` message for (`SPEC.md`, "WebSocket: session stream").
        //
        // The user is the caller's: `1000:1000` for a session, because the
        // image's `agent` account is that uid and the engine test image has no
        // account by that name. `env` and `working_dir` stay unset, because
        // `/bin/bash -l` is a login shell and the image decides both.
        let created = self
            .docker
            .create_exec(
                &id.0,
                CreateExecOptions {
                    cmd: Some(cmd.to_vec()),
                    user: Some(user.to_string()),
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| match EngineError::from(error) {
                // Podman says the same thing as Docker with a different status
                // (see `says_not_running`); both mean the terminal was asked
                // for after the session's container had gone.
                EngineError::Api { status, message } if says_not_running(&message) => {
                    debug!(container = %id, status, "the container is not running");
                    EngineError::Conflict(message)
                }
                other => other,
            })?;

        let started = self
            .docker
            .start_exec(
                &created.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: true,
                    output_capacity: None,
                }),
            )
            .await?;

        let (input, output) = match started {
            StartExecResults::Attached { input, output } => (input, output),
            // Only ever asked for with `detach: true`. An engine that detaches
            // anyway has left the terminal nothing to read or write.
            StartExecResults::Detached => {
                return Err(EngineError::Unsupported(
                    "the engine detached the terminal exec instead of attaching it".to_string(),
                ));
            }
        };

        // The size the client opened with, so the shell's first prompt is
        // already the right width. It has to follow the start: there is no PTY
        // to resize until the process has one.
        resize_exec(&self.docker, &created.id, cols, rows).await?;

        info!(container = %id, cols, rows, "started the terminal exec");
        Ok(Box::new(BollardExec::new(
            self.docker.clone(),
            created.id,
            id.clone(),
            input,
            output,
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl fmt::Display for BollardEngine {
    /// What the engine is, for a log line that has the adapter but not its
    /// fields. Never the socket, which stays out of anything formattable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind, self.version)
    }
}

#[cfg(test)]
mod tests {
    use bollard::models::{
        ContainerConfig, ContainerStateStatusEnum, EndpointSettings, ErrorDetail, NetworkSettings,
        SystemVersionComponents, SystemVersionPlatform,
    };

    use super::*;
    use crate::engine::{LABEL_PROJECT_ID, LABEL_SESSION_ID};

    /// A `/version` answer with the given components and platform name.
    fn version_reporting(components: &[&str], platform: Option<&str>) -> SystemVersion {
        SystemVersion {
            components: Some(
                components
                    .iter()
                    .map(|name| SystemVersionComponents {
                        name: (*name).to_string(),
                        version: "0.0.0-fake".to_string(),
                        details: None,
                    })
                    .collect(),
            ),
            platform: platform.map(|name| SystemVersionPlatform {
                name: name.to_string(),
            }),
            version: Some("0.0.0-fake".to_string()),
            ..Default::default()
        }
    }

    /// An inspect answer with the state, networks and labels a session
    /// container carries.
    fn inspect_response(
        status: ContainerStateStatusEnum,
        exit_code: i64,
        pid: i64,
    ) -> ContainerInspectResponse {
        ContainerInspectResponse {
            id: Some("c0ffee".to_string()),
            // The engine prefixes the name with a slash.
            name: Some("/mars-session-00000000-0000-4000-8000-00000000abcd".to_string()),
            config: Some(ContainerConfig {
                labels: Some(HashMap::from([
                    (
                        LABEL_SESSION_ID.to_string(),
                        "00000000-0000-4000-8000-00000000abcd".to_string(),
                    ),
                    (LABEL_PROJECT_ID.to_string(), "p1".to_string()),
                ])),
                ..Default::default()
            }),
            state: Some(BollardContainerState {
                status: Some(status),
                exit_code: Some(exit_code),
                pid: Some(pid),
                oom_killed: Some(false),
                ..Default::default()
            }),
            network_settings: Some(NetworkSettings {
                networks: Some(HashMap::from([
                    ("mars-sessions".to_string(), EndpointSettings::default()),
                    ("mars-egress".to_string(), EndpointSettings::default()),
                ])),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn every_state_string_the_engine_reports_has_a_state() {
        assert_eq!(container_state_of("created", 0), ContainerState::Created);
        assert_eq!(container_state_of("running", 0), ContainerState::Running);
        assert_eq!(container_state_of("paused", 0), ContainerState::Paused);
        assert_eq!(
            container_state_of("exited", 143),
            ContainerState::Exited { code: 143 }
        );
        assert_eq!(container_state_of("removing", 0), ContainerState::Removing);
        assert_eq!(container_state_of("dead", 0), ContainerState::Dead);
    }

    #[test]
    fn a_state_string_this_version_does_not_know_is_kept_verbatim() {
        // `restarting` and the engine's empty string are both states the plain
        // types do not name; neither is silently turned into something else.
        assert_eq!(
            container_state_of("restarting", 0),
            ContainerState::Unknown("restarting".to_string())
        );
        assert_eq!(
            container_state_of("", 0),
            ContainerState::Unknown(String::new())
        );
        assert_eq!(
            container_state_of("hibernating", 0),
            ContainerState::Unknown("hibernating".to_string())
        );
    }

    #[test]
    fn the_enums_string_is_the_one_the_mapping_matches_on() {
        // The mapping is written against the API's lower-case strings, so the
        // enum has to yield exactly those.
        for (status, expected) in [
            (ContainerStateStatusEnum::CREATED, ContainerState::Created),
            (ContainerStateStatusEnum::RUNNING, ContainerState::Running),
            (ContainerStateStatusEnum::PAUSED, ContainerState::Paused),
            (
                ContainerStateStatusEnum::EXITED,
                ContainerState::Exited { code: 0 },
            ),
            (ContainerStateStatusEnum::REMOVING, ContainerState::Removing),
            (ContainerStateStatusEnum::DEAD, ContainerState::Dead),
        ] {
            let state = BollardContainerState {
                status: Some(status),
                ..Default::default()
            };
            assert_eq!(container_state_of(&status_string(&state), 0), expected);
        }
    }

    #[test]
    fn an_inspect_answer_becomes_the_plain_container_info() {
        let info = to_container_info(inspect_response(ContainerStateStatusEnum::EXITED, 143, 0));

        assert_eq!(info.id, ContainerId("c0ffee".to_string()));
        // The leading slash is the engine's, not the container's name.
        assert_eq!(
            info.name,
            "mars-session-00000000-0000-4000-8000-00000000abcd"
        );
        assert_eq!(info.state, ContainerState::Exited { code: 143 });
        // Sorted, so the answer does not depend on the engine's hash order.
        assert_eq!(info.networks, vec!["mars-egress", "mars-sessions"]);
        assert_eq!(
            info.labels.get(LABEL_SESSION_ID).map(String::as_str),
            Some("00000000-0000-4000-8000-00000000abcd")
        );
        // Pid 0 is what the engine reports for a container that is not
        // running, and is not a pid.
        assert_eq!(info.pid, None);
    }

    #[test]
    fn a_running_container_reports_its_pid() {
        let info = to_container_info(inspect_response(ContainerStateStatusEnum::RUNNING, 0, 4711));

        assert_eq!(info.state, ContainerState::Running);
        assert!(info.state.is_running());
        assert_eq!(info.pid, Some(4711));
    }

    #[test]
    fn an_inspect_answer_that_says_almost_nothing_still_maps() {
        let info = to_container_info(ContainerInspectResponse::default());

        assert_eq!(info.id, ContainerId(String::new()));
        assert_eq!(info.name, "");
        assert_eq!(info.state, ContainerState::Unknown(String::new()));
        assert!(info.networks.is_empty());
        assert!(info.labels.is_empty());
        assert_eq!(info.pid, None);
    }

    #[test]
    fn only_an_exited_state_yields_the_code_a_silent_wait_falls_back_to() {
        let exited = BollardContainerState {
            status: Some(ContainerStateStatusEnum::EXITED),
            exit_code: Some(130),
            ..Default::default()
        };
        assert_eq!(exited_code(Some(&exited)), Some(130));

        let running = BollardContainerState {
            status: Some(ContainerStateStatusEnum::RUNNING),
            exit_code: Some(0),
            ..Default::default()
        };
        assert_eq!(exited_code(Some(&running)), None);
        assert_eq!(exited_code(None), None);
    }

    #[test]
    fn a_list_row_is_running_only_when_the_engine_says_running() {
        let row = |state: Option<ContainerSummaryStateEnum>| BollardContainerSummary {
            id: Some("c0ffee".to_string()),
            names: Some(vec!["/mars-session-1".to_string()]),
            labels: Some(HashMap::from([(
                LABEL_SESSION_ID.to_string(),
                "s1".to_string(),
            )])),
            state,
            ..Default::default()
        };

        let running = to_container_summary(row(Some(ContainerSummaryStateEnum::RUNNING)));
        assert!(running.running);
        assert_eq!(running.id, ContainerId("c0ffee".to_string()));
        assert_eq!(running.name, "mars-session-1");
        assert_eq!(
            running.labels.get(LABEL_SESSION_ID).map(String::as_str),
            Some("s1")
        );

        for state in [
            Some(ContainerSummaryStateEnum::CREATED),
            Some(ContainerSummaryStateEnum::EXITED),
            Some(ContainerSummaryStateEnum::PAUSED),
            Some(ContainerSummaryStateEnum::RESTARTING),
            Some(ContainerSummaryStateEnum::REMOVING),
            Some(ContainerSummaryStateEnum::DEAD),
            Some(ContainerSummaryStateEnum::EMPTY),
            None,
        ] {
            assert!(
                !to_container_summary(row(state)).running,
                "unexpectedly running: {state:?}"
            );
        }
    }

    #[test]
    fn a_list_row_without_a_name_is_still_a_row() {
        let summary = to_container_summary(BollardContainerSummary::default());

        assert_eq!(summary.name, "");
        assert!(summary.labels.is_empty());
        assert!(!summary.running);
    }

    #[test]
    fn only_an_already_exists_refusal_counts_as_already_connected() {
        assert!(says_already(
            "endpoint with name mars-session-1 already exists in network mars-egress"
        ));
        // Podman's wording, and the upper case an engine might use.
        assert!(says_already(
            "container is Already connected to network mars-egress"
        ));

        assert!(!says_already("network mars-egress not found"));
        assert!(!says_already("permission denied"));
    }

    /// Both engines refuse an exec on a container that has gone, and only one
    /// of them uses a status that already says so. The wordings are the ones
    /// they were seen to answer with.
    #[test]
    fn both_engines_refusals_of_an_exec_on_a_stopped_container_are_recognised() {
        assert!(says_not_running(
            "Container 0123456789ab is not running: exited"
        ));
        assert!(says_not_running(
            "can only create exec sessions on running containers: container state improper"
        ));

        assert!(!says_not_running("No such container: mars-session-1"));
        assert!(!says_not_running("permission denied"));
    }

    /// Both engines refuse to remove a running container without `force`, and
    /// only one of them uses a status that already says so. The wordings are
    /// the ones they were seen to answer with.
    #[test]
    fn both_engines_refusals_of_a_removal_without_force_are_recognised() {
        assert!(says_needs_force(
            "You cannot remove a running container 0123456789ab. Stop the container before \
             attempting removal or force remove"
        ));
        assert!(says_needs_force(
            "cannot remove container 0123456789ab as it is running - running or paused containers \
             cannot be removed without force: container state improper"
        ));

        assert!(!says_needs_force("No such container: mars-session-1"));
        // The exec refusal shares Podman's `state improper` tail and must not
        // be read as this one.
        assert!(!says_needs_force(
            "can only create exec sessions on running containers: container state improper"
        ));
    }

    /// Both engines refuse a create whose name is taken, and not every Podman
    /// uses a status that already says so. The wordings are the ones they were
    /// seen to answer with.
    #[test]
    fn both_engines_refusals_of_a_name_in_use_are_recognised() {
        assert!(says_name_in_use(
            "Conflict. The container name \"/mars-session-1\" is already in use by container \
             \"0123456789ab\". You have to remove (or rename) that container to be able to reuse \
             that name."
        ));
        assert!(says_name_in_use(
            "container create: creating container storage: the container name \
             \"mars-session-1\" is already in use by 0123456789ab. You have to remove that \
             container to be able to reuse that name: that name is already in use"
        ));

        assert!(!says_name_in_use(
            "No such image: mars-session-claude:latest"
        ));
        // A network that is already in use is another refusal altogether.
        assert!(!says_name_in_use("network mars-sessions is already in use"));
    }

    #[test]
    fn a_pull_stream_error_item_carries_the_registrys_own_message() {
        let info = CreateImageInfo {
            error_detail: Some(ErrorDetail {
                code: Some(1),
                message: Some("manifest for mars-session-claude:nope not found".to_string()),
            }),
            ..Default::default()
        };
        assert_eq!(
            pull_error_of(&info).as_deref(),
            Some("manifest for mars-session-claude:nope not found")
        );

        let coded = CreateImageInfo {
            error_detail: Some(ErrorDetail {
                code: Some(404),
                message: None,
            }),
            ..Default::default()
        };
        assert_eq!(
            pull_error_of(&coded).as_deref(),
            Some("the registry answered 404")
        );

        // Ordinary progress is not an error.
        let progress = CreateImageInfo {
            status: Some("Downloading".to_string()),
            ..Default::default()
        };
        assert_eq!(pull_error_of(&progress), None);
        assert_eq!(pull_error_of(&CreateImageInfo::default()), None);
    }

    #[test]
    fn a_podman_component_makes_it_podman() {
        let version = version_reporting(&["Podman Engine", "Conmon", "OCI Runtime (crun)"], None);
        assert_eq!(engine_kind_of(&version), EngineKind::Podman);
    }

    #[test]
    fn the_component_name_is_matched_without_case() {
        let version = version_reporting(&["podman engine"], None);
        assert_eq!(engine_kind_of(&version), EngineKind::Podman);
    }

    #[test]
    fn a_podman_platform_name_makes_it_podman_even_without_a_component() {
        let version = SystemVersion {
            components: None,
            platform: Some(SystemVersionPlatform {
                name: "podman".to_string(),
            }),
            ..Default::default()
        };
        assert_eq!(engine_kind_of(&version), EngineKind::Podman);
    }

    #[test]
    fn a_docker_version_is_docker() {
        let version = version_reporting(
            &["Engine", "containerd", "runc", "docker-init"],
            Some("Docker Engine - Community"),
        );
        assert_eq!(engine_kind_of(&version), EngineKind::Docker);
    }

    #[test]
    fn an_engine_that_names_nothing_is_docker() {
        assert_eq!(
            engine_kind_of(&SystemVersion::default()),
            EngineKind::Docker
        );
        assert_eq!(
            engine_kind_of(&version_reporting(&[], None)),
            EngineKind::Docker
        );
    }

    #[test]
    fn a_404_is_not_found_a_409_is_a_conflict_and_anything_else_is_the_api() {
        let not_found = EngineError::from(bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            message: "no such network: mars-sessions".to_string(),
        });
        assert!(
            matches!(&not_found, EngineError::NotFound(message) if message.contains("mars-sessions")),
            "unexpected: {not_found:?}"
        );

        let conflict = EngineError::from(bollard::errors::Error::DockerResponseServerError {
            status_code: 409,
            message: "network already exists".to_string(),
        });
        assert!(
            matches!(&conflict, EngineError::Conflict(message) if message.contains("already exists")),
            "unexpected: {conflict:?}"
        );

        let api = EngineError::from(bollard::errors::Error::DockerResponseServerError {
            status_code: 500,
            message: "engine failure".to_string(),
        });
        assert!(
            matches!(
                &api,
                EngineError::Api { status: 500, message } if message == "engine failure"
            ),
            "unexpected: {api:?}"
        );
    }

    #[test]
    fn a_transport_failure_is_a_connection_failure() {
        let error = EngineError::from(bollard::errors::Error::IOError {
            err: std::io::Error::other("connection refused"),
        });
        assert!(
            matches!(&error, EngineError::Connection(message) if message.contains("connection refused")),
            "unexpected: {error:?}"
        );

        let timeout = EngineError::from(bollard::errors::Error::RequestTimeoutError);
        assert!(
            matches!(timeout, EngineError::Connection(_)),
            "unexpected: {timeout:?}"
        );
    }

    #[tokio::test]
    async fn an_unsupported_scheme_is_refused_before_anything_is_connected() {
        for host in ["ssh://builder.example.invalid", "", "/run/docker.sock"] {
            let error = BollardEngine::connect(host)
                .await
                .expect_err("the scheme is not one of the supported three");

            assert!(
                matches!(&error, EngineError::Connection(message) if message.contains("DOCKER_HOST")),
                "unexpected for {host:?}: {error:?}"
            );
        }
    }

    #[test]
    fn the_rejection_names_the_scheme_and_nothing_else() {
        let error =
            connect_client("ssh://builder.example.invalid/run/podman.sock").expect_err("refused");
        let message = error.to_string();

        assert!(message.contains("ssh"), "the scheme is missing: {message}");
        assert!(
            !message.contains("builder.example.invalid"),
            "the address leaked: {message}"
        );

        let bare = connect_client("").expect_err("refused").to_string();
        assert!(bare.contains("(none)"), "unexpected: {bare}");
    }

    #[test]
    fn only_a_version_complaint_triggers_the_renegotiation() {
        assert!(is_api_version_mismatch(
            &bollard::errors::Error::DockerResponseServerError {
                status_code: 400,
                message: "client version 1.44 is too new. Maximum supported API version is 1.41"
                    .to_string(),
            }
        ));

        assert!(!is_api_version_mismatch(
            &bollard::errors::Error::DockerResponseServerError {
                status_code: 400,
                message: "invalid reference format".to_string(),
            }
        ));

        assert!(!is_api_version_mismatch(
            &bollard::errors::Error::DockerResponseServerError {
                status_code: 500,
                message: "client version 1.44 is too new".to_string(),
            }
        ));

        assert!(!is_api_version_mismatch(
            &bollard::errors::Error::RequestTimeoutError
        ));
    }
}
