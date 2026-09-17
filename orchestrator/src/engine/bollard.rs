//! The production [`ContainerEngine`](super::ContainerEngine) implementation,
//! on `bollard` against the Docker-compatible API (ADR 0004).
//!
//! It is the only module in the crate that may name a `bollard` type:
//! everything else speaks the plain types in [`super::types`], and every
//! `bollard::errors::Error` becomes an [`EngineError`] through the single
//! [`From`] implementation below.
//!
//! This task builds the adapter's foundations: connecting to the socket named
//! by `DOCKER_HOST`, deciding once whether the engine is Podman or Docker,
//! answering the health endpoint's ping, and creating the two session networks
//! (`ARCHITECTURE.md`, "Networks"). The container operations are stubs that
//! answer [`EngineError::Unsupported`] until the tasks that write them.
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
use std::fmt;
// Shadows the prelude's one-parameter `Result<T>` alias, exactly as
// `engine/mod.rs` does, so the signatures below read as the trait declares
// them: `Result<T, EngineError>`.
use std::result::Result;

use async_trait::async_trait;
use bollard::models::{NetworkCreateRequest, SystemVersion};
use bollard::query_parameters::InspectNetworkOptions;
use bollard::{API_DEFAULT_VERSION, Docker};

use super::{
    ContainerEngine, ContainerId, ContainerInfo, ContainerSpec, ContainerSummary, EngineError,
    EngineKind, ExecSession, ExitStatus, Signal, StdinWriter,
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
        })
    }

    /// The engine's own version string, as `/version` reported it.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The answer every operation this task does not implement yet gives.
    ///
    /// [`EngineError::Unsupported`] rather than a panic or a silent success:
    /// a caller that reaches one of these before its task lands fails loudly
    /// and says which operation it wanted.
    fn not_yet<T>(operation: &str) -> Result<T, EngineError> {
        Err(EngineError::Unsupported(format!(
            "the bollard engine cannot {operation} yet"
        )))
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

    async fn image_exists(&self, _image: &str) -> Result<bool, EngineError> {
        Self::not_yet("look up an image")
    }

    async fn pull_image(&self, _image: &str) -> Result<(), EngineError> {
        Self::not_yet("pull an image")
    }

    async fn create(&self, _spec: &ContainerSpec) -> Result<ContainerId, EngineError> {
        Self::not_yet("create a container")
    }

    async fn connect_network(&self, _id: &ContainerId, _network: &str) -> Result<(), EngineError> {
        Self::not_yet("connect a container to a network")
    }

    async fn start(&self, _id: &ContainerId) -> Result<(), EngineError> {
        Self::not_yet("start a container")
    }

    async fn stop(&self, _id: &ContainerId, _grace_secs: u32) -> Result<(), EngineError> {
        Self::not_yet("stop a container")
    }

    async fn kill(&self, _id: &ContainerId, _signal: Signal) -> Result<(), EngineError> {
        Self::not_yet("signal a container")
    }

    async fn remove(&self, _id: &ContainerId, _force: bool) -> Result<(), EngineError> {
        Self::not_yet("remove a container")
    }

    async fn inspect(&self, _id: &ContainerId) -> Result<ContainerInfo, EngineError> {
        Self::not_yet("inspect a container")
    }

    async fn wait(&self, _id: &ContainerId) -> Result<ExitStatus, EngineError> {
        Self::not_yet("wait for a container")
    }

    async fn list_by_label(&self, _label_key: &str) -> Result<Vec<ContainerSummary>, EngineError> {
        Self::not_yet("list containers")
    }

    async fn attach_stdin(&self, _id: &ContainerId) -> Result<Box<dyn StdinWriter>, EngineError> {
        Self::not_yet("attach to a container's stdin")
    }

    async fn exec_pty(
        &self,
        _id: &ContainerId,
        _cmd: &[String],
        _user: &str,
        _cols: u16,
        _rows: u16,
    ) -> Result<Box<dyn ExecSession>, EngineError> {
        Self::not_yet("start an exec")
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
    use bollard::models::{SystemVersionComponents, SystemVersionPlatform};

    use super::*;

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
