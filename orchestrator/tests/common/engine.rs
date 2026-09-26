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
//! **Two runs on one engine.** Two runs of the suite may share an engine
//! socket — two worktrees, or a coordinator beside a task-implementer — and
//! neither may disturb the other. Every container carries [`LABEL_TEST`] with
//! a value naming the process that created it ([`run_id`]), and the sweep at
//! the start of a run removes only the containers of processes that are no
//! longer running, so a crashed run's leftovers still go and a live run's
//! containers stay. The two scenarios whose assertions no label can scope —
//! the startup probe's and the image pull's — take a [`host_lock`], a `flock`
//! every run on the host shares.
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
use mars_orchestrator::ws::terminal::TerminalOutput;
use tempfile::TempDir;
use tokio::sync::{Mutex, OnceCell, mpsc};

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

/// The value of the [`LABEL_TEST`] label for this process:
/// `<host>:<boot id>:<pid>:<start time>:<uuid>`.
///
/// The first four fields name the process that created the container, so the
/// stale-container sweep below can tell a crashed run's leftovers from the
/// containers of a run that is still going on the same engine socket — a
/// second worktree, or the coordinator beside a task-implementer. The start
/// time is the process's start in clock ticks since boot (`/proc/<pid>/stat`,
/// field 22), which is what makes a reused pid read as a different process;
/// the boot id makes a pid from before a reboot read as dead. The uuid keeps
/// the value unique whatever the rest reads.
pub fn run_id() -> &'static str {
    static RUN_ID: OnceLock<String> = OnceLock::new();
    RUN_ID.get_or_init(|| {
        let pid = std::process::id();
        let start = start_time(pid).unwrap_or(0);
        format!(
            "{}:{}:{pid}:{start}:{}",
            this_host(),
            this_boot(),
            uuid::Uuid::new_v4()
        )
    })
}

/// How old a container must be before the sweep removes it when it cannot ask
/// whether its run is alive: one created from another host sharing this
/// engine, or one labelled before the label named its process. No run of the
/// suite lasts an hour.
const UNCHECKABLE_AFTER: chrono::TimeDelta = chrono::TimeDelta::hours(1);

/// A `/proc/sys/kernel` value with no `:` in it, or `unknown`.
fn kernel_value(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty() && !value.contains(':'))
        .unwrap_or_else(|| "unknown".to_string())
}

/// This host's name, as the kernel has it.
pub fn this_host() -> String {
    kernel_value("/proc/sys/kernel/hostname")
}

/// This boot's id, which changes on every reboot.
pub fn this_boot() -> String {
    kernel_value("/proc/sys/kernel/random/boot_id")
}

/// The start time of a process in clock ticks since boot, or `None` when
/// `/proc` does not say.
pub fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_start_time(&stat)
}

/// Field 22 of a `/proc/<pid>/stat` line. The command name in field 2 is in
/// parentheses and may itself hold spaces or parentheses, so the fields are
/// counted from the last `)`: what follows it starts at field 3.
pub fn parse_start_time(stat: &str) -> Option<u64> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// What the sweep makes of one container's [`LABEL_TEST`] value.
#[derive(Debug, PartialEq, Eq)]
pub enum Owner {
    /// This process: a scenario of this run created it.
    ThisRun,
    /// A process on this host that is still running.
    Alive,
    /// A process on this host that is gone, or one from an earlier boot.
    Dead,
    /// A value this host cannot check: another host's run, or a label from
    /// before the label named its process.
    Unknown,
}

/// Decide whose container a [`LABEL_TEST`] value names.
///
/// `alive` answers for a pid and start time on this host.
pub fn owner_of(label: &str, alive: impl Fn(u32, u64) -> bool) -> Owner {
    if label == run_id() {
        return Owner::ThisRun;
    }
    let fields: Vec<&str> = label.splitn(5, ':').collect();
    let [host, boot, pid, start, _uuid] = fields.as_slice() else {
        return Owner::Unknown;
    };
    let (Ok(pid), Ok(start)) = (pid.parse::<u32>(), start.parse::<u64>()) else {
        return Owner::Unknown;
    };
    if *host != this_host() {
        return Owner::Unknown;
    }
    if *boot != this_boot() {
        return Owner::Dead;
    }
    if alive(pid, start) {
        Owner::Alive
    } else {
        Owner::Dead
    }
}

/// Whether the process `pid`, started at `start`, is still running here.
///
/// The test processes run on this host even when the engine is rootless, so
/// the pid in the label is one of this host's. A start time of 0 is a run that
/// could not read its own, and falls back to the pid alone, as `common::db`
/// does. A `/proc` entry that exists but cannot be read counts as alive:
/// removing a live run's containers is the failure this exists to prevent,
/// and a leftover is only a leftover.
pub fn process_alive(pid: u32, start: u64) -> bool {
    if start != 0 {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => return parse_start_time(&stat) == Some(start),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
            Err(_) => {}
        }
    }
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 is the documented existence check and sends nothing.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    // `EPERM` is another user's process, which is alive; only `ESRCH` says the
    // pid is free.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
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

/// Remove the leftovers of runs that are over: every container labelled
/// [`LABEL_TEST`] whose creating process is no longer running — a crashed or
/// killed earlier run — and one whose process cannot be checked once it is
/// older than [`UNCHECKABLE_AFTER`].
///
/// Containers of the *current* run are left alone, because scenarios run in
/// parallel threads and one of them may have created its container already;
/// so are those of every other run still alive on this host, because two runs
/// of `tests/engine.rs` may share one engine socket and must not remove each
/// other's containers mid-scenario (Bears 4q3t2).
pub async fn cleanup_stale_test_containers(engine: &BollardEngine) {
    let stale = match engine.list_by_label(LABEL_TEST).await {
        Ok(containers) => containers,
        Err(error) => {
            eprintln!("could not list stale test containers: {error}");
            return;
        }
    };

    let now = chrono::Utc::now();
    for container in stale {
        let label = container.labels.get(LABEL_TEST).map_or("", String::as_str);
        let remove = match owner_of(label, process_alive) {
            Owner::ThisRun | Owner::Alive => false,
            Owner::Dead => true,
            Owner::Unknown => now - container.created > UNCHECKABLE_AFTER,
        };
        if !remove {
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

/// Hold an engine-wide fact still across every run of the suite on this host.
///
/// Two scenarios assert something no label can scope to their own run:
/// `bootstrap_engine_end_to_end` that no container carries the probe's
/// `mars.probe` label once it is done, and `pull_absent_image` that an image
/// it removed is absent. A second run on the same engine — another worktree —
/// would break either from outside the process, so the lock is an exclusive
/// `flock` on a file of the temporary directory named after `name` and the
/// user, not a mutex: every call opens its own file description, so it
/// serialises the threads of this process and every other process alike. It is
/// released when the returned file is dropped, and by the kernel when a run is
/// killed holding it.
pub async fn host_lock(name: &str) -> std::fs::File {
    // SAFETY: `getuid` cannot fail and has no preconditions.
    let uid = unsafe { libc::getuid() };
    let path = std::env::temp_dir().join(format!("mars-engine-test-{name}-{uid}.lock"));

    tokio::task::spawn_blocking(move || {
        use std::os::fd::AsRawFd;

        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .expect("the engine test lock file opens");
        // SAFETY: the descriptor is open for as long as `file` lives, and
        // `flock` touches nothing but its lock.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        assert_eq!(
            locked,
            0,
            "the engine test lock {} was not taken: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
        file
    })
    .await
    .expect("the lock task does not panic")
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
/// `bootstrap_engine_end_to_end` — use the session's own `1000:1000` instead.
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

/// A PTY's bytes with the parts that are not text taken out: ANSI escape
/// sequences and carriage returns.
///
/// A login shell writes more than the answer to the question it was asked —
/// `\r\n` line endings always, and on a shell that thinks it is interactive a
/// cursor movement or a colour — and an assertion on the raw bytes would be an
/// assertion on which shell the image happens to ship. Searching this instead
/// keeps the scenarios about the terminal contract.
pub fn plain_text(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {}
            '\u{1b}' => match chars.peek() {
                // CSI: parameters, then one final byte in `@`..=`~`.
                Some('[') => {
                    chars.next();
                    for parameter in chars.by_ref() {
                        if ('@'..='~').contains(&parameter) {
                            break;
                        }
                    }
                }
                // OSC: a string ended by BEL or ST.
                Some(']') => {
                    chars.next();
                    while let Some(byte) = chars.next() {
                        if byte == '\u{7}' {
                            break;
                        }
                        if byte == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                // Anything else two bytes long, or a stray escape at the end.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            _ => out.push(ch),
        }
    }

    out
}

/// Read [`TerminalOutput::Data`] chunks until [`plain_text`] of everything
/// seen so far contains `needle`, and hand back the raw bytes.
///
/// The failure is a message rather than a panic so a scenario can use this to
/// *probe* — `terminal_adapter_login_shell_as_uid_1000` falls back to another
/// user when the login shell does not come up under `1000:1000` on one engine
/// — and [`collect_terminal_output`] is the panicking form every other wait
/// uses.
///
/// A terminal that closes, or detaches, before the needle arrives is an error
/// and not a wait that runs out: that is the more useful message.
pub async fn try_collect_terminal_output(
    rx: &mut mpsc::Receiver<TerminalOutput>,
    needle: &str,
    timeout: std::time::Duration,
) -> std::result::Result<Vec<u8>, String> {
    let mut seen: Vec<u8> = Vec::new();

    let outcome = tokio::time::timeout(timeout, async {
        loop {
            match rx.recv().await {
                Some(TerminalOutput::Data(chunk)) => {
                    seen.extend_from_slice(&chunk);
                    if plain_text(&seen).contains(needle) {
                        return Ok(());
                    }
                }
                Some(TerminalOutput::Closed { exit_code }) => {
                    return Err(format!(
                        "the terminal closed with exit code {exit_code} before {needle:?} arrived"
                    ));
                }
                None => {
                    return Err(format!("the terminal detached before {needle:?} arrived"));
                }
            }
        }
    })
    .await;

    match outcome {
        Ok(Ok(())) => Ok(seen),
        Ok(Err(reason)) => Err(format!("{reason}; saw: {:?}", plain_text(&seen))),
        Err(_) => Err(format!(
            "{needle:?} did not arrive within {timeout:?}; saw: {:?}",
            plain_text(&seen)
        )),
    }
}

/// [`try_collect_terminal_output`], as an assertion.
pub async fn collect_terminal_output(
    rx: &mut mpsc::Receiver<TerminalOutput>,
    needle: &str,
    timeout: std::time::Duration,
) -> Vec<u8> {
    match try_collect_terminal_output(rx, needle, timeout).await {
        Ok(seen) => seen,
        Err(reason) => panic!("{reason}"),
    }
}

/// Discard output until the terminal ends by itself, and hand back the exit
/// code it reported (`SPEC.md`, "WebSocket: session stream":
/// `terminal_closed { exit_code }`).
pub async fn await_terminal_closed(
    rx: &mut mpsc::Receiver<TerminalOutput>,
    timeout: std::time::Duration,
) -> i64 {
    let mut seen: Vec<u8> = Vec::new();

    let outcome = tokio::time::timeout(timeout, async {
        loop {
            match rx.recv().await {
                Some(TerminalOutput::Data(chunk)) => seen.extend_from_slice(&chunk),
                Some(TerminalOutput::Closed { exit_code }) => return Some(exit_code),
                None => return None,
            }
        }
    })
    .await;

    match outcome {
        Ok(Some(exit_code)) => exit_code,
        Ok(None) => panic!(
            "the terminal detached without reporting a close; saw: {:?}",
            plain_text(&seen)
        ),
        Err(_) => panic!(
            "the terminal did not close within {timeout:?}; saw: {:?}",
            plain_text(&seen)
        ),
    }
}

/// `path` as an absolute path, which is what a bind-mount source and
/// `DATA_DIR_HOST` both have to be.
pub fn absolute(path: &std::path::Path) -> PathBuf {
    path.canonicalize().expect("the path resolves")
}
