//! The startup probe: a short-lived container run with the same `HostConfig` a
//! session would get, proving that a file it writes under `DATA_DIR` comes
//! back owned by the orchestrator's own uid.
//!
//! It is what verifies that Podman honours `keep-id` and that the bind mounts
//! and uid layout are sane on either engine (`ARCHITECTURE.md`, "Engine
//! adapter", Startup probe; ADR 0004). A failure is
//! [`EngineError::Probe`](super::EngineError::Probe) and is fatal at startup,
//! because every session would otherwise fail later in less obvious ways.
//!
//! **Why the file is enough.** The probe container runs as uid 1000 inside its
//! user namespace and touches one file on a bind mount the orchestrator can
//! see. Three things can be wrong with the result and each has its own
//! message: the file is missing, which on macOS means `DATA_DIR_HOST` is not
//! shared with the engine's VM; it is there but owned by somebody else, which
//! under rootless Podman means `keep-id:uid=1000,gid=1000` was not honoured
//! and the owner is a sub-uid; or it is owned correctly and still not
//! writable. Only the owning uid is compared — the group may legitimately
//! differ under `keep-id`.
//!
//! **Who owns "the orchestrator's uid".** The probe directory the orchestrator
//! created itself a moment earlier is the reference, so the answer needs no
//! `libc` call: whoever owns that directory is who the orchestrator runs as.
//!
//! **Secrets.** The probe spec carries no environment at all (see
//! [`build_probe_spec`]), so nothing here can log one (CLAUDE.md rule 3).

use std::fs::{self, DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
// Shadows the prelude's one-parameter `Result<T>` alias, as every module under
// `engine/` does: the probe reports the engine's own [`EngineError`].
use std::result::Result;
use std::time::{Duration, Instant};

use super::spec::{ProbeSpecInput, build_probe_spec};
use super::{ContainerEngine, ContainerId, EngineError, EngineKind, Signal};
// The crate convention (`CLAUDE.md`, "Backend conventions"); the `info!` and
// `warn!` macros come from here.
use crate::prelude::*;

/// How long the probe container is given to touch one file and exit.
///
/// Generous on purpose: the container is `touch` and nothing else, so the
/// whole budget is the engine's own create-and-start latency on a loaded
/// machine. Reaching it means the engine is wedged, not that the probe is
/// slow.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// The file the probe container touches, relative to its `work` directory.
///
/// The container side of this name is the `cmd` in
/// [`build_probe_spec`](super::spec::build_probe_spec) — `touch
/// /session/work/probe-ok` — and the two have to agree: that bind's host
/// source is the directory read here.
const PROBE_FILE: &str = "probe-ok";

/// The subdirectories of a probe directory, which are the session directories
/// the spec binds into the container.
///
/// `home` and `log` are mounted although the probe writes only under `work`,
/// because the image's entrypoint redirects its output into `/session/log` and
/// takes `/session/home` as `HOME` before it runs anything: a probe that
/// mounted only `work` would not be running what a session runs
/// (`ARCHITECTURE.md`, "Session container specification").
const PROBE_SUBDIRS: [&str; 3] = ["work", "home", "log"];

/// The mode the probe directory itself is created with, matching the session
/// directories the launcher creates.
///
/// It stays the ordinary mode because nothing writes into it: the check reads
/// the orchestrator's own uid off this directory and the container writes one
/// level down, in [`PROBE_SUBDIR_MODE`].
const PROBE_DIR_MODE: u32 = 0o755;

/// The mode the three session subdirectories are given after creation.
///
/// World-writable on purpose, so that the check measures ownership and not
/// permission (`ARCHITECTURE.md`, "Engine adapter", Startup probe). Under
/// Docker the container's uid 1000 is uid 1000 on the host, so against a
/// 0o755 directory owned by an orchestrator running as some other uid the
/// container cannot write its file at all and the probe reports an exit code
/// instead of the uid mismatch that actually caused it. These directories are
/// throwaway — created under `DATA_DIR/tmp`, removed after the probe, swept by
/// orphan cleanup if the process dies — so the mode weakens nothing.
const PROBE_SUBDIR_MODE: u32 = 0o777;

/// Everything the probe needs that `Config` decides.
///
/// Both data paths are here because they are not the same directory seen
/// twice: `data_dir` is where the orchestrator itself looks at the file
/// afterwards, and `data_dir_host` is what the engine resolves the bind-mount
/// source against (`ARCHITECTURE.md`, "Storage"). They are equal only when the
/// orchestrator runs on the host.
#[derive(Debug, Clone)]
pub struct ProbeInput {
    /// `Config::session_image_default`: the image new projects' default
    /// profile uses, so the probe proves the layout for the image sessions
    /// will actually run.
    pub image: String,
    /// `DATA_DIR`, the path at which the orchestrator sees the volume.
    pub data_dir: PathBuf,
    /// `DATA_DIR_HOST`, the path the engine resolves bind-mount sources
    /// against.
    pub data_dir_host: PathBuf,
    /// `SESSION_NETWORK_INTERNAL`, the network the container is created on.
    pub network_internal: String,
    /// `SESSION_NETWORK_EGRESS`, connected before start exactly as a session's
    /// is.
    pub network_egress: String,
    /// `SESSION_EXTRA_HOSTS`, already parsed into `host:ip` entries.
    pub extra_hosts: Vec<String>,
}

/// What a passing probe proved, for the caller's log line and for the engine
/// tests to assert on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeReport {
    /// The engine the probe ran on, which decides whether `keep-id` was part
    /// of what was proved.
    pub engine_kind: EngineKind,
    /// The uid owning the file the container wrote. Equal to
    /// [`own_uid`](Self::own_uid) on success; the two are reported separately
    /// because their inequality is the failure this whole exercise exists to
    /// catch.
    pub file_uid: u32,
    /// The uid the orchestrator runs as, read off the directory it created
    /// itself.
    pub own_uid: u32,
    /// How long the whole probe took, pull included.
    pub duration: Duration,
}

/// Run the startup probe and report what it proved.
///
/// Creates `DATA_DIR/tmp/probe-<random>/{work,home,log}`, runs one container
/// from `input.image` over it with the session `HostConfig`, and checks the
/// file that container wrote. The container is removed on every path, the
/// directory is removed afterwards, and neither removal failing turns a pass
/// into a failure — `/data/tmp` and stray `mars.probe` containers are what
/// orphan cleanup is for (`ARCHITECTURE.md`, "Background jobs").
///
/// A pass logs one `info!` line. A failure is returned, not logged: the caller
/// is startup, which logs it once at `error!` and stops.
///
/// # Errors
///
/// [`EngineError::Probe`] when the container ran but did not prove what it has
/// to prove — a non-zero exit, a timeout, a missing file, a file owned by
/// another uid, a file that cannot be appended to, or a pull that failed. Any
/// other variant is the engine itself failing and is returned unchanged, so
/// the operator sees "the engine is unreachable" as that rather than as a
/// probe verdict.
pub async fn run_startup_probe(
    engine: &dyn ContainerEngine,
    input: ProbeInput,
) -> Result<ProbeReport, EngineError> {
    let started = Instant::now();
    // 64 bits of randomness names both the directory and the container, so two
    // orchestrators sharing a data directory and an engine cannot collide on
    // either.
    let suffix = format!("{:016x}", rand::random::<u64>());
    let probe_dir = probe_dir_in(&input.data_dir, &suffix);

    create_probe_dirs(&probe_dir)?;
    let outcome = run_probe_container(engine, &input, &suffix, &probe_dir).await;

    // Unconditional: a failed probe stops startup, and leaving its directory
    // behind would only grow `/data/tmp` across restart attempts. A directory
    // whose contents the orchestrator may not own — precisely the uid-mismatch
    // failure — cannot be removed, which is a `warn!` and not the verdict.
    if let Err(error) = fs::remove_dir_all(&probe_dir) {
        warn!(
            path = %probe_dir.display(),
            %error,
            "could not remove the startup probe directory"
        );
    }

    // On success the two uids are equal by definition: the check is what
    // returned the value.
    let uid = outcome?;
    let report = ProbeReport {
        engine_kind: engine.kind(),
        file_uid: uid,
        own_uid: uid,
        duration: started.elapsed(),
    };

    info!(
        engine_kind = %report.engine_kind,
        image = %input.image,
        uid = report.own_uid,
        duration_ms = report.duration.as_millis() as u64,
        "startup probe passed"
    );
    Ok(report)
}

/// Everything between the directory being there and the verdict, so that the
/// caller can clean the directory up on every path with one `if let`.
///
/// Returns the uid that owns both the probe directory and the file the
/// container wrote.
async fn run_probe_container(
    engine: &dyn ContainerEngine,
    input: &ProbeInput,
    suffix: &str,
    probe_dir: &Path,
) -> Result<u32, EngineError> {
    ensure_image(engine, &input.image).await?;

    let spec = build_probe_spec(&ProbeSpecInput {
        suffix: suffix.to_string(),
        image: input.image.clone(),
        probe_dir_host: probe_dir_in(&input.data_dir_host, suffix),
        network_internal: input.network_internal.clone(),
        extra_hosts: input.extra_hosts.clone(),
    });

    let id = engine.create(&spec).await?;
    let outcome = run_to_exit(engine, &id, &input.network_egress).await;

    // Before the verdict and on every path, including the ones that never
    // started it: a probe container that survives its probe is an orphan.
    match engine.remove(&id, true).await {
        // Already gone is already removed.
        Ok(()) | Err(EngineError::NotFound(_)) => {}
        Err(error) => warn!(
            container_id = %id,
            %error,
            "could not remove the startup probe container; orphan cleanup will take it"
        ),
    }

    outcome?;
    check_probe_file(probe_dir, &probe_dir.join("work").join(PROBE_FILE))
}

/// Connect the egress network, start the container and wait for it, with the
/// same network order a session launch uses: the container never runs with the
/// wrong set of networks.
async fn run_to_exit(
    engine: &dyn ContainerEngine,
    id: &ContainerId,
    network_egress: &str,
) -> Result<(), EngineError> {
    engine.connect_network(id, network_egress).await?;
    engine.start(id).await?;

    match tokio::time::timeout(PROBE_TIMEOUT, engine.wait(id)).await {
        Ok(Ok(status)) if status.code == 0 => Ok(()),
        Ok(Ok(status)) => Err(EngineError::Probe(format!(
            "probe container exited with code {}",
            status.code
        ))),
        Ok(Err(error)) => Err(error),
        Err(_elapsed) => {
            // The removal that follows is forced and would kill it anyway;
            // signalling first means the engine is asked to end a process it
            // still believes in, which is the friendlier order and the one
            // whose failure is worth a line.
            if let Err(error) = engine.kill(id, Signal::Sigkill).await {
                warn!(
                    container_id = %id,
                    %error,
                    "could not kill the timed-out startup probe container"
                );
            }
            Err(EngineError::Probe(format!(
                "probe container did not exit within {}s",
                PROBE_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Pull the image if the engine does not have it, which is the ordinary state
/// of a first start.
///
/// A pull failure becomes a probe failure carrying the registry's own words,
/// because at startup there is no session row to put them in and the operator
/// needs to read "manifest unknown" rather than "the probe failed".
async fn ensure_image(engine: &dyn ContainerEngine, image: &str) -> Result<(), EngineError> {
    if engine.image_exists(image).await? {
        return Ok(());
    }

    info!(%image, "pulling default session image");
    match engine.pull_image(image).await {
        Ok(()) => Ok(()),
        Err(EngineError::ImagePull { message, .. }) => Err(EngineError::Probe(format!(
            "probe image pull failed: {message}"
        ))),
        Err(error) => Err(error),
    }
}

/// `<data>/tmp/probe-<suffix>`, in whichever of the two views of the data
/// directory is passed.
fn probe_dir_in(data: &Path, suffix: &str) -> PathBuf {
    data.join("tmp").join(format!("probe-{suffix}"))
}

/// Create the probe directory and its three session subdirectories, and
/// `DATA_DIR/tmp` along the way.
fn create_probe_dirs(probe_dir: &Path) -> Result<(), EngineError> {
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(PROBE_DIR_MODE);

    for subdir in PROBE_SUBDIRS {
        let path = probe_dir.join(subdir);
        builder.create(&path)?;
        // Separately from the builder's mode, which the process umask masks:
        // the container has to be able to write here whatever umask the
        // service was started with.
        fs::set_permissions(&path, fs::Permissions::from_mode(PROBE_SUBDIR_MODE))?;
    }
    Ok(())
}

/// The whole verdict: the file is there, the orchestrator owns it, and the
/// orchestrator can write to it.
///
/// `probe_dir` is the reference for "the orchestrator's uid" — it created that
/// directory itself moments ago — and `file` is what the container wrote.
/// Returns the uid both are owned by.
fn check_probe_file(probe_dir: &Path, file: &Path) -> Result<u32, EngineError> {
    let own_uid = fs::metadata(probe_dir)?.uid();

    // Any failure to stat the file is "it is not there": the directory was
    // stat-ed a line earlier, so a permission error on a child of it is not a
    // distinction the operator can act on differently. The common cause is a
    // `DATA_DIR_HOST` the engine's VM does not share, where the container
    // wrote its file into a directory nobody else can see.
    let Ok(metadata) = fs::metadata(file) else {
        return Err(EngineError::Probe(format!(
            "probe file was not written: {}",
            file.display()
        )));
    };

    let file_uid = metadata.uid();
    if file_uid != own_uid {
        // Both numbers, because which one is surprising depends on the engine:
        // a sub-uid like 100999 is Podman ignoring `keep-id`, and a file owned
        // by 1000 under an orchestrator that is not 1000 is the documented
        // Docker contract not being met.
        return Err(EngineError::Probe(format!(
            "probe file is owned by uid {file_uid}, orchestrator runs as uid {own_uid}: \
             on Podman check that keep-id:uid=1000,gid=1000 is supported, \
             on Docker run the orchestrator as uid 1000 with DATA_DIR_HOST owned by uid 1000"
        )));
    }

    // Ownership is not permission: a file the container wrote 0o444 would pass
    // the uid check and still stop every session that tried to edit it.
    // Appending nothing changes nothing.
    OpenOptions::new()
        .append(true)
        .open(file)
        .map_err(|error| {
            EngineError::Probe(format!(
                "probe file is not writable by the orchestrator: {error}"
            ))
        })?;

    Ok(own_uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The happy filesystem: the orchestrator owns what it finds and can
    /// append to it.
    #[test]
    fn a_file_the_orchestrator_owns_and_can_append_to_passes() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let file = dir.path().join(PROBE_FILE);
        fs::write(&file, b"").expect("the probe file is written");

        let uid = check_probe_file(dir.path(), &file).expect("the check passes");
        assert_eq!(
            uid,
            fs::metadata(dir.path())
                .expect("the directory is there")
                .uid(),
            "the uid reported is the one owning the directory the orchestrator made"
        );
    }

    /// The macOS failure: the container wrote its file somewhere the
    /// orchestrator cannot see, so the directory is empty.
    #[test]
    fn a_file_that_never_arrived_names_the_path_it_was_expected_at() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let file = dir.path().join(PROBE_FILE);

        let error = check_probe_file(dir.path(), &file).expect_err("the check fails");
        assert_eq!(
            probe_reason(&error),
            format!("probe file was not written: {}", file.display())
        );
    }

    /// Owned correctly and still unusable.
    #[test]
    fn a_file_the_orchestrator_cannot_append_to_fails() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let file = dir.path().join(PROBE_FILE);
        fs::write(&file, b"").expect("the probe file is written");

        let mut mode = fs::metadata(&file)
            .expect("the file is there")
            .permissions();
        mode.set_readonly(true);
        fs::set_permissions(&file, mode).expect("the file is made read-only");

        // Root ignores the mode bits, so there is nothing to assert as root.
        if fs::metadata(dir.path())
            .expect("the directory is there")
            .uid()
            == 0
        {
            return;
        }

        let error = check_probe_file(dir.path(), &file).expect_err("the check fails");
        assert!(
            probe_reason(&error).starts_with("probe file is not writable by the orchestrator: "),
            "unexpected: {error}"
        );
    }

    /// The failure the whole probe exists for, and the one message an operator
    /// has to be able to act on: it names both uids and what to check on
    /// either engine.
    ///
    /// The reference directory is `/`, owned by root, standing in for a
    /// `DATA_DIR` the orchestrator does not own — chowning a file to another
    /// uid is exactly the privilege a test does not have, which is also why
    /// the live version of this path is an engine test.
    #[test]
    fn a_file_owned_by_another_uid_names_both_uids_and_both_engines() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let file = dir.path().join(PROBE_FILE);
        fs::write(&file, b"").expect("the probe file is written");

        let file_uid = fs::metadata(&file).expect("the file is there").uid();
        // As root the two uids would agree and there would be no mismatch.
        if file_uid == 0 {
            return;
        }

        let error = check_probe_file(Path::new("/"), &file).expect_err("the check fails");
        assert_eq!(
            probe_reason(&error),
            format!(
                "probe file is owned by uid {file_uid}, orchestrator runs as uid 0: \
                 on Podman check that keep-id:uid=1000,gid=1000 is supported, \
                 on Docker run the orchestrator as uid 1000 with DATA_DIR_HOST owned by uid 1000"
            )
        );
    }

    /// The subdirectories the container writes into are world-writable
    /// whatever the umask, so that a host which does not honour the uid
    /// contract fails on the ownership check with the message naming both
    /// uids, rather than on a write the container was never allowed to make.
    #[test]
    fn the_probe_subdirectories_are_world_writable() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let probe_dir = probe_dir_in(dir.path(), "0123456789abcdef");

        create_probe_dirs(&probe_dir).expect("the probe directories are created");

        for subdir in PROBE_SUBDIRS {
            let mode = fs::metadata(probe_dir.join(subdir))
                .expect("the subdirectory is there")
                .mode()
                & 0o777;
            assert_eq!(
                mode, PROBE_SUBDIR_MODE,
                "{subdir} is not writable by the container's uid"
            );
        }

        // The directory the uid check reads its reference off is not part of
        // that, and is left at the ordinary session mode.
        let mode = fs::metadata(&probe_dir)
            .expect("the probe directory is there")
            .mode()
            & 0o777;
        assert_eq!(
            mode & 0o002,
            0,
            "the probe directory itself should not be world-writable: {mode:o}"
        );
    }

    /// The reason inside an [`EngineError::Probe`], so a test can compare the
    /// exact string the operator will read.
    fn probe_reason(error: &EngineError) -> String {
        match error {
            EngineError::Probe(reason) => reason.clone(),
            other => panic!("expected a probe failure, got: {other:?}"),
        }
    }
}

/// The control flow around the container, driven by the mock engine.
///
/// The mock has no filesystem, so a test that wants the probe to pass writes
/// `work/probe-ok` itself — from the spec's own bind, which is also how it
/// checks that the bind the container would have written through is the
/// directory the orchestrator then reads.
#[cfg(all(test, feature = "integration-tests"))]
mod engine_tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::engine::mock::MockEngine;
    use crate::engine::types::{
        ContainerInfo, ContainerSpec, ContainerState, ContainerSummary, ExecSession, ExitStatus,
        StdinWriter,
    };

    /// The mock mints ids in creation order and the probe creates exactly one
    /// container.
    const PROBE_CONTAINER: &str = "mock-0";

    /// A [`MockEngine`] that also keeps an ordered log of the calls the probe's
    /// contract is about.
    ///
    /// The mock forgets a container's signals when it is removed, which is
    /// exactly the pair — kill, then remove — the timeout path has to be
    /// checked for, so the log lives outside the container table.
    struct RecordingEngine {
        inner: Arc<MockEngine>,
        calls: Mutex<Vec<String>>,
    }

    impl RecordingEngine {
        fn new() -> Self {
            Self {
                inner: Arc::new(MockEngine::default()),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// The mock underneath, which the test's writer task drives.
        fn mock(&self) -> Arc<MockEngine> {
            Arc::clone(&self.inner)
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("the call log is healthy").clone()
        }

        fn record(&self, call: impl Into<String>) {
            self.calls
                .lock()
                .expect("the call log is healthy")
                .push(call.into());
        }
    }

    #[async_trait]
    impl ContainerEngine for RecordingEngine {
        fn kind(&self) -> EngineKind {
            self.inner.kind()
        }

        async fn ping(&self) -> Result<(), EngineError> {
            self.inner.ping().await
        }

        async fn ensure_network(&self, name: &str, internal: bool) -> Result<(), EngineError> {
            self.inner.ensure_network(name, internal).await
        }

        async fn image_exists(&self, image: &str) -> Result<bool, EngineError> {
            self.record("image_exists");
            self.inner.image_exists(image).await
        }

        async fn pull_image(&self, image: &str) -> Result<(), EngineError> {
            self.record("pull_image");
            self.inner.pull_image(image).await
        }

        async fn create(&self, spec: &ContainerSpec) -> Result<ContainerId, EngineError> {
            self.record("create");
            self.inner.create(spec).await
        }

        async fn connect_network(
            &self,
            id: &ContainerId,
            network: &str,
        ) -> Result<(), EngineError> {
            self.record(format!("connect_network {network}"));
            self.inner.connect_network(id, network).await
        }

        async fn start(&self, id: &ContainerId) -> Result<(), EngineError> {
            self.record("start");
            self.inner.start(id).await
        }

        async fn stop(&self, id: &ContainerId, grace_secs: u32) -> Result<(), EngineError> {
            self.inner.stop(id, grace_secs).await
        }

        async fn kill(&self, id: &ContainerId, signal: Signal) -> Result<(), EngineError> {
            self.record(format!("kill {signal}"));
            self.inner.kill(id, signal).await
        }

        async fn remove(&self, id: &ContainerId, force: bool) -> Result<(), EngineError> {
            self.record(format!("remove force={force}"));
            self.inner.remove(id, force).await
        }

        async fn inspect(&self, id: &ContainerId) -> Result<ContainerInfo, EngineError> {
            self.inner.inspect(id).await
        }

        async fn wait(&self, id: &ContainerId) -> Result<ExitStatus, EngineError> {
            self.record("wait");
            self.inner.wait(id).await
        }

        async fn list_by_label(
            &self,
            label_key: &str,
        ) -> Result<Vec<ContainerSummary>, EngineError> {
            self.inner.list_by_label(label_key).await
        }

        async fn attach_stdin(
            &self,
            id: &ContainerId,
        ) -> Result<Box<dyn StdinWriter>, EngineError> {
            self.inner.attach_stdin(id).await
        }

        async fn exec_pty(
            &self,
            id: &ContainerId,
            cmd: &[String],
            user: &str,
            cols: u16,
            rows: u16,
        ) -> Result<Box<dyn ExecSession>, EngineError> {
            self.inner.exec_pty(id, cmd, user, cols, rows).await
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn probe_input(data: &Path) -> ProbeInput {
        ProbeInput {
            // Obviously fake, and never pulled: the mock answers
            // `image_exists` with `true` unless a test says otherwise.
            image: "mars-session-claude:test".to_string(),
            data_dir: data.to_path_buf(),
            // The orchestrator runs on the host in this test, where the two
            // views of the data directory are the same path.
            data_dir_host: data.to_path_buf(),
            network_internal: "mars-sessions".to_string(),
            network_egress: "mars-egress".to_string(),
            extra_hosts: Vec::new(),
        }
    }

    fn probe_id() -> ContainerId {
        ContainerId(PROBE_CONTAINER.to_string())
    }

    /// Park until the probe has started its container, which is when the mock
    /// will accept an `exit` for it.
    async fn started(engine: &MockEngine) -> ContainerId {
        let id = probe_id();
        loop {
            if engine.state_of(&id) == Some(ContainerState::Running) {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// The host directory behind the container's `/session/work`, which is
    /// where the real container's `touch` would land.
    fn work_dir(engine: &MockEngine, id: &ContainerId) -> PathBuf {
        engine
            .spec_of(id)
            .expect("the probe container exists")
            .binds
            .iter()
            .find(|bind| bind.container_target == "/session/work")
            .expect("the probe mounts a work directory")
            .host_source
            .clone()
    }

    /// The whole passing path: pull nothing, create, connect the egress
    /// network, start, wait, remove, and find the file where the bind said it
    /// would be.
    #[tokio::test]
    async fn a_probe_whose_file_comes_back_owned_by_the_orchestrator_passes() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = RecordingEngine::new();

        let mock = engine.mock();
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            fs::write(work_dir(&mock, &id).join(PROBE_FILE), b"")
                .expect("the probe file is written");
            assert!(mock.exit(&id, 0), "the probe container is still there");
        });

        let report = run_startup_probe(&engine, probe_input(data.path()))
            .await
            .expect("the probe passes");
        container.await.expect("the container task finished");

        assert_eq!(report.engine_kind, EngineKind::Podman);
        assert_eq!(report.own_uid, report.file_uid);
        assert_eq!(
            engine.calls(),
            vec![
                "image_exists",
                "create",
                "connect_network mars-egress",
                "start",
                "wait",
                "remove force=true",
            ]
        );
        assert_eq!(
            engine.inner.state_of(&probe_id()),
            None,
            "the probe container is removed"
        );
        assert!(
            fs::read_dir(data.path().join("tmp"))
                .expect("DATA_DIR/tmp was created")
                .next()
                .is_none(),
            "the probe directory is removed"
        );
    }

    /// The first start of a fresh host: the image is not there yet.
    #[tokio::test]
    async fn a_missing_image_is_pulled_before_the_container_is_created() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = RecordingEngine::new();
        engine
            .inner
            .set_missing_images(["mars-session-claude:test"]);

        let mock = engine.mock();
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            fs::write(work_dir(&mock, &id).join(PROBE_FILE), b"")
                .expect("the probe file is written");
            mock.exit(&id, 0);
        });

        run_startup_probe(&engine, probe_input(data.path()))
            .await
            .expect("the probe passes");
        container.await.expect("the container task finished");

        assert_eq!(
            engine.inner.pulled_images(),
            vec!["mars-session-claude:test".to_string()]
        );
        assert_eq!(
            engine.calls().first().map(String::as_str),
            Some("image_exists")
        );
        assert_eq!(
            engine.calls().get(1).map(String::as_str),
            Some("pull_image")
        );
    }

    /// A pull failure carries the registry's own words and never reaches the
    /// container steps.
    #[tokio::test]
    async fn a_failed_pull_is_a_probe_failure_carrying_the_engine_message() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = RecordingEngine::new();
        engine
            .inner
            .set_missing_images(["mars-session-claude:test"]);
        engine.inner.fail_next_pull("manifest unknown");

        let error = run_startup_probe(&engine, probe_input(data.path()))
            .await
            .expect_err("the probe fails");

        assert_eq!(
            reason(&error),
            "probe image pull failed: manifest unknown".to_string()
        );
        assert_eq!(engine.calls(), vec!["image_exists", "pull_image"]);
    }

    /// A container that ran and failed is reported by its code, and is still
    /// removed.
    #[tokio::test]
    async fn a_non_zero_exit_fails_the_probe_and_the_container_is_still_removed() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = RecordingEngine::new();

        let mock = engine.mock();
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            mock.exit(&id, 127);
        });

        let error = run_startup_probe(&engine, probe_input(data.path()))
            .await
            .expect_err("the probe fails");
        container.await.expect("the container task finished");

        assert_eq!(reason(&error), "probe container exited with code 127");
        assert!(
            engine.calls().contains(&"remove force=true".to_string()),
            "unexpected calls: {:?}",
            engine.calls()
        );
        assert_eq!(engine.inner.state_of(&probe_id()), None);
        assert!(
            fs::read_dir(data.path().join("tmp"))
                .expect("DATA_DIR/tmp was created")
                .next()
                .is_none(),
            "a failed probe removes its directory too"
        );
    }

    /// A container that exits cleanly without writing anything is the unshared
    /// `DATA_DIR_HOST`: the check runs and names the path it looked at.
    #[tokio::test]
    async fn a_clean_exit_that_wrote_nothing_is_the_missing_file_failure() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = RecordingEngine::new();

        let mock = engine.mock();
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            mock.exit(&id, 0);
        });

        let error = run_startup_probe(&engine, probe_input(data.path()))
            .await
            .expect_err("the probe fails");
        container.await.expect("the container task finished");

        let message = reason(&error);
        assert!(
            message.starts_with("probe file was not written: "),
            "unexpected: {message}"
        );
        assert!(
            message.ends_with("/work/probe-ok"),
            "the path names the file the container should have touched: {message}"
        );
        assert_eq!(engine.inner.state_of(&probe_id()), None);
    }

    /// A container that never exits is killed and removed, and the message
    /// names the budget it blew.
    ///
    /// Time is paused, so the runtime advances its own clock to the deadline
    /// the moment every task is parked; the test costs no wall-clock seconds
    /// and still exercises the real [`PROBE_TIMEOUT`].
    #[tokio::test(start_paused = true)]
    async fn a_container_that_never_exits_is_killed_and_removed() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = RecordingEngine::new();

        let error = run_startup_probe(&engine, probe_input(data.path()))
            .await
            .expect_err("the probe fails");

        assert_eq!(reason(&error), "probe container did not exit within 60s");
        assert_eq!(
            engine.calls(),
            vec![
                "image_exists",
                "create",
                "connect_network mars-egress",
                "start",
                "wait",
                "kill SIGKILL",
                "remove force=true",
            ],
            "the timed-out container is signalled before it is removed"
        );
        assert_eq!(engine.inner.state_of(&probe_id()), None);
    }

    /// An engine failure is not a probe verdict: the variant is passed
    /// through, and the container is removed all the same.
    #[tokio::test]
    async fn a_container_that_disappears_propagates_not_found() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = RecordingEngine::new();

        let mock = engine.mock();
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            assert!(mock.vanish(&id), "the probe container is still there");
        });

        let error = run_startup_probe(&engine, probe_input(data.path()))
            .await
            .expect_err("the probe fails");
        container.await.expect("the container task finished");

        assert!(
            matches!(error, EngineError::NotFound(_)),
            "unexpected: {error:?}"
        );
        assert!(
            engine.calls().contains(&"remove force=true".to_string()),
            "unexpected calls: {:?}",
            engine.calls()
        );
    }

    fn reason(error: &EngineError) -> String {
        match error {
            EngineError::Probe(reason) => reason.clone(),
            other => panic!("expected a probe failure, got: {other:?}"),
        }
    }
}
