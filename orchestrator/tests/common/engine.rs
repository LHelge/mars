//! Helpers for the live engine suite in `tests/engine.rs` (`CLAUDE.md`,
//! "Testing expectations": the engine tests run only when `DOCKER_HOST` is
//! set).
//!
//! Everything here exists so that `tests/engine.rs` reads as the operation
//! table of `ARCHITECTURE.md`, "Engine adapter", one scenario per row:
//! connecting or skipping, the throwaway container specification, the unique
//! names and labels that let two runs share an engine, and the cleanup that
//! runs whether the scenario passed or panicked.
//!
//! Nothing in this module talks to the database, so it compiles without the
//! `integration-tests` feature like `common::db` does.
//!
//! **Secrets.** No container built here carries any environment at all, and
//! nothing is ever a real credential (CLAUDE.md rule 3).

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use bollard::query_parameters::RemoveImageOptionsBuilder;
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::FutureExt;
use mars_orchestrator::engine::bollard::BollardEngine;
use mars_orchestrator::engine::{Bind, ContainerEngine, ContainerId, ContainerSpec};
use tempfile::TempDir;
use tokio::sync::{Mutex, OnceCell};

/// The image every scenario but `pull_absent_image` runs.
///
/// Fully qualified, because Podman resolves an unqualified name against its
/// configured registries and Docker against Docker Hub; naming the registry
/// makes the two agree.
pub const TEST_IMAGE: &str = "docker.io/library/alpine:3.20";

/// The image `pull_absent_image` removes and pulls back.
///
/// A tag of its own, so removing it cannot race a scenario that is about to
/// create a container from [`TEST_IMAGE`].
pub const PULL_TEST_IMAGE: &str = "docker.io/library/alpine:3.19";

/// The label every container this suite creates carries, with the run id as
/// its value: what tells one run's leftovers from another's.
pub const LABEL_TEST: &str = "mars.test";

/// The network a scenario that is not about networking creates its container
/// on. The engine's own default bridge exists on both engines.
pub const TEST_NETWORK: &str = "bridge";

/// How long any single `wait` in the suite may take.
pub const WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The value of the [`LABEL_TEST`] label for this process.
///
/// Random per run, so the stale-container sweep below can remove every
/// `mars.test` container that is *not* this run's without touching a suite
/// running beside it.
pub fn run_id() -> &'static str {
    static RUN_ID: OnceLock<String> = OnceLock::new();
    RUN_ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// `DOCKER_HOST`, if it names anything.
pub fn docker_host() -> Option<String> {
    std::env::var("DOCKER_HOST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The engine every scenario starts with, or `None` when there is no engine to
/// test against.
///
/// `CLAUDE.md`, "Testing expectations": with `DOCKER_HOST` unset the whole
/// suite is skipped, and a skipped scenario passes — the tests are reported as
/// passing and return before touching an engine. A `DOCKER_HOST` that is set
/// and does not answer is a failure, not a skip: that is a broken environment,
/// not an absent one.
pub async fn connect_or_skip() -> Option<BollardEngine> {
    let Some(host) = docker_host() else {
        eprintln!("DOCKER_HOST not set; skipping engine test");
        return None;
    };

    let engine = BollardEngine::connect(&host)
        .await
        .expect("the engine named by DOCKER_HOST answers");

    // Once per process, before the first scenario creates anything.
    static SWEPT: OnceCell<()> = OnceCell::const_new();
    SWEPT
        .get_or_init(|| cleanup_stale_test_containers(&engine))
        .await;

    Some(engine)
}

/// A `bollard` client of this suite's own.
///
/// [`BollardEngine`] deliberately does not expose its client — the adapter is
/// the boundary (ADR 0004) — and two operations the cleanup needs are not
/// operations Mars performs and so are not on the trait: removing an image and
/// removing a network. They are done here, against the same socket, in the
/// same way `BollardEngine::connect` opens one.
pub fn raw_docker() -> Docker {
    let host = docker_host().expect("DOCKER_HOST is set");

    if let Some(path) = host.strip_prefix("unix://") {
        Docker::connect_with_socket(path, 120, API_DEFAULT_VERSION)
    } else {
        Docker::connect_with_http(&host, 120, API_DEFAULT_VERSION)
    }
    .expect("a client for DOCKER_HOST")
}

/// Remove every container labelled [`LABEL_TEST`] with a value other than this
/// run's: the leftovers of a crashed earlier run.
///
/// Containers of the *current* run are left alone, because scenarios run in
/// parallel threads and one of them may have created its container already.
async fn cleanup_stale_test_containers(engine: &BollardEngine) {
    let stale = match engine.list_by_label(LABEL_TEST).await {
        Ok(containers) => containers,
        Err(error) => {
            eprintln!("could not list stale test containers: {error}");
            return;
        }
    };

    for container in stale {
        if container.labels.get(LABEL_TEST).map(String::as_str) == Some(run_id()) {
            continue;
        }
        if let Err(error) = engine.remove(&container.id, true).await {
            eprintln!(
                "could not remove the stale test container {}: {error}",
                container.name
            );
        }
    }
}

/// Pull [`TEST_IMAGE`] if the engine does not have it.
///
/// Serialised process-wide: every scenario calls this and they run in parallel
/// threads, so without the lock several would start the same pull.
pub async fn ensure_test_image(engine: &BollardEngine) {
    static PULL: Mutex<()> = Mutex::const_new(());
    let _guard = PULL.lock().await;

    let present = engine
        .image_exists(TEST_IMAGE)
        .await
        .expect("the engine answers whether the test image is present");
    if !present {
        engine
            .pull_image(TEST_IMAGE)
            .await
            .expect("the test image pulls");
    }
}

/// Remove an image if the engine has it, through the raw client.
///
/// Used only by `pull_absent_image`, which is the one scenario that needs an
/// image to be absent.
pub async fn remove_image_if_present(image: &str) {
    let docker = raw_docker();
    let options = RemoveImageOptionsBuilder::default().force(true).build();

    // A missing image is the state this asks for, so its failure is not one.
    if let Err(error) = docker.remove_image(image, Some(options), None).await {
        eprintln!("could not remove {image} (it may be absent already): {error}");
    }
}

/// The two probe scenarios share one thing no label can separate: the probe
/// container's own `mars.probe` label, whose absence afterwards they assert.
/// They take this lock so only one probe is ever in flight.
pub fn probe_lock() -> &'static Mutex<()> {
    static PROBE: Mutex<()> = Mutex::const_new(());
    &PROBE
}

/// `mars-test-<scenario>-<random>`: a container or network name no other run,
/// and no other scenario, can collide with.
pub fn unique_name(scenario: &str) -> String {
    format!("mars-test-{scenario}-{:08x}", rand::random::<u32>())
}

/// The specification every generic scenario starts from: this suite's label,
/// the test image, the engine's default bridge, no environment, no binds.
///
/// `user` is `0:0` rather than `1000:1000` on purpose. Alpine has no account
/// at uid 1000, and under rootless Podman without `keep-id` the container's
/// root is the host user, so a container that writes into a temporary
/// directory produces files the test can read whichever engine it ran on. The
/// two scenarios where ownership is the point — `userns_keep_id_accepted` and
/// `startup_probe_end_to_end` — use the session's own `1000:1000` instead.
pub fn test_spec(name: &str, cmd: &[&str]) -> ContainerSpec {
    ContainerSpec {
        image: TEST_IMAGE.to_string(),
        name: name.to_string(),
        labels: BTreeMap::from([(LABEL_TEST.to_string(), run_id().to_string())]),
        user: "0:0".to_string(),
        working_dir: "/".to_string(),
        cmd: cmd.iter().map(|part| (*part).to_string()).collect(),
        env: Vec::new(),
        secret_env: Vec::new(),
        binds: Vec::new(),
        network: TEST_NETWORK.to_string(),
        extra_hosts: Vec::new(),
        runtime: None,
    }
}

/// A read-write bind of a host directory at a container path.
pub fn rw_bind(host_source: &std::path::Path, container_target: &str) -> Bind {
    Bind {
        host_source: host_source.to_path_buf(),
        container_target: container_target.to_string(),
        read_only: false,
    }
}

/// A temporary directory a container may write into whatever uid it runs as.
///
/// 0o777 because the container's user is not the test's user on every engine:
/// a GitHub-hosted Docker runner is uid 1001 and the container is 1000, and
/// under rootless Podman with `keep-id` the container's 1000 is the host user.
/// The mode is set explicitly rather than left to the umask.
pub fn writable_tempdir() -> TempDir {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777))
        .expect("the temporary directory is made world-writable");
    dir
}

/// The uid that owns a path: who the test process runs as, read the way the
/// startup probe reads it.
pub fn uid_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::MetadataExt;

    std::fs::metadata(path).expect("the path is there").uid()
}

/// What a scenario has created and what therefore has to go, whether the
/// scenario passed or panicked.
///
/// Registered as it goes rather than collected at the end, so a panic halfway
/// through still leaves a complete list. Containers are removed before
/// networks, because a network with a container still attached cannot be
/// removed.
#[derive(Default)]
pub struct Cleanup {
    containers: StdMutex<Vec<ContainerId>>,
    networks: StdMutex<Vec<String>>,
}

impl Cleanup {
    /// Remove this container when the scenario ends.
    pub fn container(&self, id: &ContainerId) {
        self.containers
            .lock()
            .expect("the cleanup list is not poisoned")
            .push(id.clone());
    }

    /// Remove this network when the scenario ends.
    pub fn network(&self, name: &str) {
        self.networks
            .lock()
            .expect("the cleanup list is not poisoned")
            .push(name.to_string());
    }

    /// Do it, and report what could not be done.
    async fn run(&self, engine: &BollardEngine) -> Vec<String> {
        let mut failures = Vec::new();

        let containers = std::mem::take(
            &mut *self
                .containers
                .lock()
                .expect("the cleanup list is not poisoned"),
        );
        for id in containers {
            if let Err(error) = engine.remove(&id, true).await {
                failures.push(format!("container {id} was not removed: {error}"));
            }
        }

        let networks = std::mem::take(
            &mut *self
                .networks
                .lock()
                .expect("the cleanup list is not poisoned"),
        );
        if !networks.is_empty() {
            let docker = raw_docker();
            for name in networks {
                if let Err(error) = docker.remove_network(&name).await {
                    failures.push(format!("network {name} was not removed: {error}"));
                }
            }
        }

        failures
    }
}

/// Run a scenario's body and clean up after it whatever it did.
///
/// A `Drop` guard cannot await, so the cleanup is a wrapper instead: the body
/// is caught, the containers and networks it registered are removed, and only
/// then is a panic resumed. A cleanup failure is itself a failure — a leftover
/// `mars-test-*` container or network is a bug in the suite — but only when
/// the body passed, because a body that panicked has the more interesting
/// message and must not have it replaced.
pub async fn with_cleanup<F, Fut>(engine: &BollardEngine, body: F)
where
    F: FnOnce(Arc<Cleanup>) -> Fut,
    Fut: Future<Output = ()>,
{
    // Scenarios run in parallel. They used to take a mutex here, because
    // Podman cannot resolve `keep-id` for two containers at once, but that is
    // now the adapter's own guarantee: `BollardEngine::create` holds a
    // per-engine-host lock across the create call (`ARCHITECTURE.md`, "Engine
    // adapter", the `UsernsMode` row; Bears u6zkz). Each scenario connects its
    // own `BollardEngine`, but the lock is shared per `DOCKER_HOST` within the
    // process, so the adapter covers them all;
    // `concurrent_session_creates_all_start` is the scenario that asserts it.
    let cleanup = Arc::new(Cleanup::default());
    let outcome = AssertUnwindSafe(body(Arc::clone(&cleanup)))
        .catch_unwind()
        .await;
    let failures = cleanup.run(engine).await;

    match outcome {
        Err(panic) => {
            for failure in failures {
                eprintln!("cleanup after a failed scenario: {failure}");
            }
            std::panic::resume_unwind(panic)
        }
        Ok(()) => assert!(failures.is_empty(), "the scenario leaked: {failures:?}"),
    }
}

/// `path` as an absolute path, which is what a bind-mount source and
/// `DATA_DIR_HOST` both have to be.
pub fn absolute(path: &std::path::Path) -> PathBuf {
    path.canonicalize().expect("the path resolves")
}
