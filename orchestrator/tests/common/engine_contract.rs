//! The engine conformance suite: one scenario per line of the normalised
//! semantics every [`ContainerEngine`] implementation has to answer with
//! (`ARCHITECTURE.md`, "Engine adapter", Normalised semantics).
//!
//! The normalisations are the interface, not `bollard`'s private business: what
//! an operation answers for a container that is missing, one that has exited
//! and one that is already running is stated in the trait's own documentation,
//! and this module is the executable definition of it. Everything here goes
//! through `Arc<dyn ContainerEngine>` and nothing knows which adapter it is
//! driving, so a second adapter — a remote engine, a different runtime — is
//! conforming exactly when it passes.
//!
//! [`EngineContract::assert_all`] runs every scenario; each is also a method of
//! its own, so a caller can run them one at a time. `tests/engine.rs` runs the
//! suite against `BollardEngine` when `DOCKER_HOST` is set and
//! `tests/engine_mock.rs` against `MockEngine` on every run of the test suite
//! (`cargo test --features integration-tests`, the mock's own feature), where
//! every scenario runs: the mock is held to the whole list, because a mock that
//! answers something else makes every test built on it agree with the wrong
//! thing.
//!
//! **What is not here.** Two lines of the list need a state the trait cannot
//! arrange: an image the engine does not have, which `tests/engine.rs`
//! (`pull_absent_image`) removes through a raw client and `MockEngine` arranges
//! with `set_missing_images`, and a pull that fails, which the mock arms with
//! `fail_next_pull`. The suite asserts the half it can reach — an image that is
//! there is `Ok(true)` — and leaves those two to the adapter's own tests.
//!
//! **Waits are bounded.** Every [`ContainerEngine::wait`] here runs under a
//! timeout, so an adapter that parks forever where the contract says it answers
//! fails the scenario instead of hanging the run.
//!
//! **Secrets.** No container built here carries any environment at all, and
//! nothing is ever a real credential (CLAUDE.md rule 3).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerSpec, ContainerState, EngineError, ExitStatus,
    LABEL_SESSION_ID, Signal,
};
use tokio::io::AsyncWriteExt;

use super::engine::{LABEL_TEST, run_id, unique_name};

/// The command every container in the suite runs: it stays up until it is
/// signalled and then exits on a code that says which signal arrived.
///
/// The traps are not decoration. A process that is PID 1 of a pid namespace
/// receives a signal from outside that namespace only if it has installed a
/// handler for it (`pid_namespaces(7)`), so a bare `sleep` would ignore both
/// and the container would never exit.
///
/// It opens with what a session image's entrypoint does for stdin — the FIFO at
/// [`STDIN_FIFO`](mars_orchestrator::engine::STDIN_FIFO), held open read-write
/// — because that is what `attach_stdin` relays into on a real engine (ADR
/// 0034); the mock needs none of it and ignores the command.
const TRAPPING_CMD: [&str; 3] = [
    "sh",
    "-c",
    r#"mkfifo /tmp/mars-stdin; exec 0<>/tmp/mars-stdin; trap "exit 143" TERM; trap "exit 42" INT; while true; do sleep 1; done"#,
];

/// The exit code [`TRAPPING_CMD`] leaves on a `SIGTERM`, which is what
/// [`ContainerEngine::stop`] sends: the code the stop sequence's hard stop
/// produces (`ARCHITECTURE.md`, "Stop semantics"), and the one an adapter that
/// only pretends to run containers reports for a stop.
const TERM_EXIT_CODE: i64 = 143;

/// The exit code [`TRAPPING_CMD`] leaves on a `SIGINT`, which is the first
/// signal of Mars's own stop sequence. Deliberately not 130, so the code is
/// evidence the trap ran rather than something the engine could have produced
/// by tearing the container down.
const INT_EXIT_CODE: i64 = 42;

/// How long the suite gives the container's command to install its traps.
const TRAP_DELAY: Duration = Duration::from_secs(1);

/// How long any single `wait` in the suite may take.
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a stopped container's stdin attachment may take to close.
///
/// The engine ends the attach stream a little after the container exits, and
/// that gap is what this covers; it is not a wait for anything to happen.
const ATTACH_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// The grace period the suite stops containers with, in seconds. Long enough
/// for the `TERM` trap to run, so the exit code is the trap's and not the
/// engine's hard kill.
const STOP_GRACE_SECS: u32 = 10;

/// What a scenario needs besides the engine itself.
///
/// The two networks have to exist already: creating them is the caller's, and
/// so is removing them, because removing a network is not an operation Mars
/// performs and therefore not one on the trait.
pub struct ContractEnv {
    /// The network containers are created on.
    pub network: String,
    /// A second network [`ContainerEngine::connect_network`] attaches before
    /// the start, as the launcher attaches the egress network.
    pub second_network: String,
    /// Called with every container the suite creates, so the caller can remove
    /// it whether the scenario passed or panicked. The suite removes what it
    /// can itself; this is the safety net for a scenario that fails halfway.
    pub container_created: Box<dyn Fn(&ContainerId) + Send + Sync>,
    /// Called with every network the suite creates, for the same reason.
    pub network_created: Box<dyn Fn(&str) + Send + Sync>,
}

impl ContractEnv {
    /// An environment for an adapter that leaves nothing behind to clean up,
    /// which is what an in-memory mock is.
    pub fn unmanaged(network: &str, second_network: &str) -> Self {
        Self {
            network: network.to_string(),
            second_network: second_network.to_string(),
            container_created: Box::new(|_| {}),
            network_created: Box::new(|_| {}),
        }
    }
}

/// One engine under the contract.
pub struct EngineContract {
    /// The adapter being held to the contract. Deliberately the trait object:
    /// nothing here may reach for a concrete engine.
    engine: Arc<dyn ContainerEngine>,
    /// The image every container is created from. Only an adapter that really
    /// runs containers cares what it is.
    image: String,
    /// The networks and the cleanup hooks.
    env: ContractEnv,
}

/// Run every scenario of the contract against one engine.
///
/// The entry point the two suites call: `tests/engine.rs` with a
/// `BollardEngine` and the networks it created, `tests/engine_mock.rs` with a
/// `MockEngine`.
pub async fn assert_engine_contract(
    engine: Arc<dyn ContainerEngine>,
    image: &str,
    env: ContractEnv,
) {
    EngineContract::new(engine, image, env).assert_all().await;
}

impl EngineContract {
    /// The contract over one engine.
    pub fn new(engine: Arc<dyn ContainerEngine>, image: &str, env: ContractEnv) -> Self {
        Self {
            engine,
            image: image.to_string(),
            env,
        }
    }

    /// Every scenario, in the order of the list in `ARCHITECTURE.md`.
    pub async fn assert_all(&self) {
        self.ping_is_reachability().await;
        self.an_image_that_is_there_is_not_an_error().await;
        self.an_existing_network_is_ok().await;
        self.create_connect_start_and_wait_in_that_order().await;
        self.a_name_in_use_is_a_conflict().await;
        self.start_of_a_running_container_is_ok().await;
        self.stop_of_an_exited_container_is_ok().await;
        self.kill_of_an_exited_container_is_a_conflict().await;
        self.remove_of_a_missing_container_is_ok().await;
        self.remove_of_a_running_container_needs_force().await;
        self.every_other_operation_on_a_missing_container_is_not_found()
            .await;
        self.list_by_label_reports_running_and_exited_containers()
            .await;
        self.a_signal_reaches_the_containers_main_process().await;
        self.a_dropped_stdin_writer_leaves_the_container_running()
            .await;
        self.a_stdin_write_after_the_container_exited_fails().await;
        self.exec_pty_on_a_container_that_is_not_running_is_a_conflict()
            .await;
    }

    // ---- the scenarios ------------------------------------------------------

    /// `ping` is reachability and nothing else.
    pub async fn ping_is_reachability(&self) {
        self.engine.ping().await.expect("the engine is reachable");
    }

    /// `image_exists` answers `Ok(true)` for an image that is there, and never
    /// an error.
    ///
    /// The absent half needs a state the trait cannot arrange; see this
    /// module's header.
    pub async fn an_image_that_is_there_is_not_an_error(&self) {
        assert!(
            self.engine
                .image_exists(&self.image)
                .await
                .expect("the engine answers whether an image is present"),
            "the image the contract runs on is not there: {}",
            self.image
        );
    }

    /// A network that already exists is `Ok` and is left exactly as it is,
    /// including when a concurrent creation is what made it exist.
    pub async fn an_existing_network_is_ok(&self) {
        let name = unique_name("contract-net");
        // Registered before it is created, so a failure halfway leaves nothing
        // behind.
        (self.env.network_created)(&name);

        self.engine
            .ensure_network(&name, true)
            .await
            .expect("the network is created");
        self.engine
            .ensure_network(&name, true)
            .await
            .expect("a network that already exists is not an error");

        // Left exactly as it is: an existing network whose own flags disagree
        // with what was asked for is still `Ok`, because a network in use is not
        // this component's to take down (`ARCHITECTURE.md`, "Networks").
        self.engine
            .ensure_network(&name, false)
            .await
            .expect("an existing network is not recreated and not an error");
    }

    /// The launch order: created on one network, connected to the second, and
    /// only then started, so the container never runs with the wrong set
    /// (`ARCHITECTURE.md`, "Networks"). The wait answers the code the container
    /// exited on, and a later inspect answers the same one.
    pub async fn create_connect_start_and_wait_in_that_order(&self) {
        let (spec, id) = self.created("contract-lifecycle").await;

        let info = self
            .engine
            .inspect(&id)
            .await
            .expect("the created container");
        assert_eq!(info.state, ContainerState::Created);
        assert_eq!(info.name, spec.name);
        assert_eq!(
            info.labels.get(LABEL_TEST).map(String::as_str),
            Some(run_id()),
            "the container did not come back with the labels it was created with"
        );
        assert_eq!(info.pid, None, "a container that is not running has no pid");

        self.engine
            .connect_network(&id, &self.env.second_network)
            .await
            .expect("the second network connects before the start");
        self.engine.start(&id).await.expect("the container starts");

        let info = self
            .engine
            .inspect(&id)
            .await
            .expect("the running container");
        assert_eq!(info.state, ContainerState::Running);
        for network in [&self.env.network, &self.env.second_network] {
            assert!(
                info.networks.contains(network),
                "the container is not on {network}: {:?}",
                info.networks
            );
        }

        self.engine
            .stop(&id, STOP_GRACE_SECS)
            .await
            .expect("a running container stops");
        let status = self.wait_within(&id).await;
        assert_eq!(
            status,
            ExitStatus {
                code: TERM_EXIT_CODE,
                oom_killed: false
            },
            "a stop ends the container on its TERM handler's code"
        );

        let info = self
            .engine
            .inspect(&id)
            .await
            .expect("the exited container");
        assert_eq!(
            info.state,
            ContainerState::Exited {
                code: TERM_EXIT_CODE
            },
            "the inspect disagrees with the wait about how the container ended"
        );

        // A second wait answers at once from the recorded exit, which is what
        // makes it safe to call on a container a restart readopted.
        assert_eq!(self.wait_within(&id).await, status);

        self.engine
            .remove(&id, false)
            .await
            .expect("an exited container is removed without force");
        self.assert_not_found(self.engine.inspect(&id).await.err(), "inspect");
    }

    /// A container name already in use is a conflict, which is how a second
    /// launch of the same session is refused.
    pub async fn a_name_in_use_is_a_conflict(&self) {
        let (spec, _id) = self.created("contract-name").await;

        let error = match self.engine.create(&spec).await {
            Ok(id) => {
                (self.env.container_created)(&id);
                panic!("the engine created a second container with the same name");
            }
            Err(error) => error,
        };
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );
    }

    /// A container that is already running is `Ok` to start again: both engines
    /// answer 304 and the adapter reads it as success, so a start that races
    /// another start cannot fail on it.
    pub async fn start_of_a_running_container_is_ok(&self) {
        let (_spec, id) = self.running("contract-restart").await;

        self.engine
            .start(&id)
            .await
            .expect("starting a running container is idempotent");

        let info = self.engine.inspect(&id).await.expect("the container");
        assert_eq!(
            info.state,
            ContainerState::Running,
            "the second start disturbed the container"
        );
    }

    /// A container that is not running is `Ok` to stop, whether it has already
    /// exited or was never started: being stopped is what the caller asked for
    /// and it already is, which is the 304 both engines answer.
    pub async fn stop_of_an_exited_container_is_ok(&self) {
        let (_spec, id) = self.exited("contract-stop-exited").await;

        self.engine
            .stop(&id, STOP_GRACE_SECS)
            .await
            .expect("stopping a container that has exited is not a failure");

        let info = self.engine.inspect(&id).await.expect("the container");
        assert_eq!(
            info.state,
            ContainerState::Exited {
                code: TERM_EXIT_CODE
            },
            "the second stop changed how the container had ended"
        );

        let (_spec, created) = self.created("contract-stop-created").await;

        self.engine
            .stop(&created, STOP_GRACE_SECS)
            .await
            .expect("stopping a container that was never started is not a failure");

        let info = self.engine.inspect(&created).await.expect("the container");
        assert_eq!(
            info.state,
            ContainerState::Created,
            "the stop started or ended a container that had never run"
        );
    }

    /// Signalling a container that has exited is a conflict, which the session
    /// owner reads as "it is already gone" rather than as a failure.
    pub async fn kill_of_an_exited_container_is_a_conflict(&self) {
        let (_spec, id) = self.exited("contract-kill-exited").await;

        let error = self
            .engine
            .kill(&id, Signal::Sigint)
            .await
            .expect_err("a container that has exited cannot be signalled");
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );
    }

    /// Removing a container that is not there is success: orphan cleanup and
    /// the end of a session both want the container gone, and it is.
    pub async fn remove_of_a_missing_container_is_ok(&self) {
        let missing = self.missing_id();

        self.engine
            .remove(&missing, true)
            .await
            .expect("a container that is already gone is already removed");
        self.engine
            .remove(&missing, false)
            .await
            .expect("force makes no difference to a container that is not there");
    }

    /// A running container is a conflict to remove without `force`, and `Ok`
    /// with it.
    pub async fn remove_of_a_running_container_needs_force(&self) {
        let (_spec, id) = self.running("contract-remove-running").await;

        let error = self
            .engine
            .remove(&id, false)
            .await
            .expect_err("a running container is not removed unasked");
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );

        self.engine
            .remove(&id, true)
            .await
            .expect("force removes a running container");
        self.assert_not_found(self.engine.inspect(&id).await.err(), "inspect");
    }

    /// Every operation but `remove` answers `NotFound` for a container the
    /// engine does not have. That variant is what recovery reads as "the
    /// container is gone".
    pub async fn every_other_operation_on_a_missing_container_is_not_found(&self) {
        let id = self.missing_id();
        let cmd = vec!["sh".to_string()];

        self.assert_not_found(self.engine.start(&id).await.err(), "start");
        self.assert_not_found(self.engine.stop(&id, 1).await.err(), "stop");
        self.assert_not_found(self.engine.kill(&id, Signal::Sigint).await.err(), "kill");
        self.assert_not_found(self.engine.inspect(&id).await.err(), "inspect");
        self.assert_not_found(
            self.engine
                .connect_network(&id, &self.env.second_network)
                .await
                .err(),
            "connect_network",
        );
        self.assert_not_found(self.engine.attach_stdin(&id).await.err(), "attach_stdin");
        // `ExecSession` is not `Debug`, so the refusal is matched rather than
        // unwrapped.
        self.assert_not_found(
            self.engine.exec_pty(&id, &cmd, "0:0", 80, 24).await.err(),
            "exec_pty",
        );

        // Bounded: a wait that parks on a container the engine does not have is
        // exactly the failure this asserts against.
        let waited = tokio::time::timeout(WAIT_TIMEOUT, self.engine.wait(&id))
            .await
            .expect("a wait on a missing container answers rather than parking");
        self.assert_not_found(waited.err(), "wait");
    }

    /// The label filter recovery runs on: every container carrying the key,
    /// running or exited, and nothing else. A key nothing carries is an empty
    /// list rather than an error.
    pub async fn list_by_label_reports_running_and_exited_containers(&self) {
        // This scenario's own value, so its two containers can be picked out of
        // an engine that is also running other sessions.
        let session_id = uuid::Uuid::new_v4().to_string();

        let mut running = self.spec("contract-listed-running");
        running
            .labels
            .insert(LABEL_SESSION_ID.to_string(), session_id.clone());
        let mut exited = self.spec("contract-listed-exited");
        exited
            .labels
            .insert(LABEL_SESSION_ID.to_string(), session_id.clone());
        let unlabelled = self.spec("contract-listed-unlabelled");

        // Both engines report `created` at one-second resolution and may round
        // down, so the window this scenario's containers have to fall in
        // starts a second before the first create.
        let before = chrono::Utc::now() - chrono::TimeDelta::seconds(1);
        let running_id = self.create(&running).await;
        let exited_id = self.create(&exited).await;
        let unlabelled_id = self.create(&unlabelled).await;

        self.engine
            .start(&running_id)
            .await
            .expect("the container starts");
        self.engine
            .start(&exited_id)
            .await
            .expect("the container starts");
        self.engine
            .stop(&exited_id, STOP_GRACE_SECS)
            .await
            .expect("the container stops");
        self.wait_within(&exited_id).await;

        let listed = self
            .engine
            .list_by_label(LABEL_SESSION_ID)
            .await
            .expect("the engine lists by label");
        let mine: Vec<_> = listed
            .iter()
            .filter(|row| row.labels.get(LABEL_SESSION_ID) == Some(&session_id))
            .collect();
        assert_eq!(
            mine.len(),
            2,
            "expected exactly the two labelled containers, got {mine:?}"
        );

        let row = |name: &str| {
            mine.iter()
                .find(|row| row.name == name)
                .unwrap_or_else(|| panic!("{name} is not listed: {mine:?}"))
        };
        assert!(
            row(&running.name).running,
            "the running container is not reported running"
        );
        assert_eq!(row(&running.name).id, running_id);
        assert!(
            !row(&exited.name).running,
            "the exited container is reported running"
        );
        assert!(
            !listed.iter().any(|row| row.name == unlabelled.name),
            "a container without the label was listed"
        );

        // `created` is the normalised creation time orphan cleanup's
        // five-minute guard is measured against (`ARCHITECTURE.md`,
        // "Background jobs"), so every adapter has to report a real one and
        // not a placeholder.
        let after = chrono::Utc::now() + chrono::TimeDelta::seconds(1);
        for name in [&running.name, &exited.name] {
            let created = row(name).created;
            assert!(
                created >= before && created <= after,
                "{name} was created just now but is listed as created at {created}",
            );
        }

        // A key nothing carries: an empty list, not a failure.
        let unknown = unique_name("contract-label");
        assert!(
            self.engine
                .list_by_label(&unknown)
                .await
                .expect("an unknown label key is not an error")
                .is_empty(),
            "a label key nothing carries listed something"
        );

        for id in [running_id, exited_id, unlabelled_id] {
            self.engine.remove(&id, true).await.expect("removed");
        }
    }

    /// A named signal reaches the container's main process, which the exit code
    /// is the evidence for: the command's `INT` trap exits on a code the engine
    /// could not have produced by tearing the container down.
    pub async fn a_signal_reaches_the_containers_main_process(&self) {
        let (_spec, id) = self.running("contract-signal").await;
        // The trap has to be installed before the signal arrives.
        tokio::time::sleep(TRAP_DELAY).await;

        self.engine
            .kill(&id, Signal::Sigint)
            .await
            .expect("a running container takes a signal");

        let status = self.wait_within(&id).await;
        assert_eq!(
            status.code, INT_EXIT_CODE,
            "SIGINT did not reach the container's main process: it exited {}",
            status.code
        );
    }

    /// Dropping the writer is not the end of the process that was reading it,
    /// and a second attach is accepted: what an orchestrator restart does to a
    /// running session (`ARCHITECTURE.md`, "Restart procedure"; ADR 0034). That
    /// both writers reach the *same* process is asserted where there is a
    /// process to ask, in `tests/engine.rs` and `tests/session_e2e.rs`.
    pub async fn a_dropped_stdin_writer_leaves_the_container_running(&self) {
        let (_spec, id) = self.running("contract-stdin-drop").await;

        let mut stdin = self
            .engine
            .attach_stdin(&id)
            .await
            .expect("a running container's stdin attaches");
        stdin.write_all(b"before\n").await.expect("the first write");
        stdin.flush().await.expect("the first flush");
        drop(stdin);

        // Longer than an EOF took to end the process when a closed attach was
        // one: about 100 ms on a rootless Podman.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let info = self
            .engine
            .inspect(&id)
            .await
            .expect("the container inspects");
        assert_eq!(
            info.state,
            ContainerState::Running,
            "dropping the stdin writer ended the container"
        );

        let mut again = self
            .engine
            .attach_stdin(&id)
            .await
            .expect("the stdin of a container that was attached before attaches again");
        again.write_all(b"after\n").await.expect("the second write");
        again.flush().await.expect("the second flush");
    }

    /// A write to a container that has exited fails rather than being silently
    /// lost: a rootless Podman accepts and discards such writes, so the
    /// adapter's own detection is what has to turn one into an error.
    pub async fn a_stdin_write_after_the_container_exited_fails(&self) {
        let (_spec, id) = self.running("contract-stdin-exit").await;

        // Attached before the exit: the attachment has to be in place for the
        // exit to be the event under test.
        let mut stdin = self
            .engine
            .attach_stdin(&id)
            .await
            .expect("a running container's stdin attaches");

        self.engine
            .stop(&id, STOP_GRACE_SECS)
            .await
            .expect("the container stops");
        self.wait_within(&id).await;

        let deadline = std::time::Instant::now() + ATTACH_CLOSE_TIMEOUT;
        let failure = loop {
            let attempt = match stdin.write_all(b"ignored\n").await {
                Ok(()) => stdin.flush().await,
                Err(error) => Err(error),
            };

            match attempt {
                Err(error) => break error,
                Ok(()) if std::time::Instant::now() >= deadline => {
                    panic!("writes to the attachment still succeed after the container exited")
                }
                Ok(()) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        };

        assert!(
            !failure.to_string().is_empty(),
            "the failed write says nothing at all"
        );
    }

    /// An exec on a container that is not running is a conflict, whether it has
    /// not been started yet or has already exited, and however the engine
    /// numbered the refusal — Docker 409, Podman 500.
    pub async fn exec_pty_on_a_container_that_is_not_running_is_a_conflict(&self) {
        let cmd = vec!["sh".to_string()];

        let (_spec, created) = self.created("contract-exec-created").await;
        let (_spec, exited) = self.exited("contract-exec-exited").await;

        for (id, state) in [(created, "created"), (exited, "exited")] {
            // `ExecSession` is not `Debug`, so the refusal is matched rather
            // than unwrapped with `expect_err`.
            let Some(error) = self.engine.exec_pty(&id, &cmd, "0:0", 80, 24).await.err() else {
                panic!("an exec in a container that is {state} was accepted");
            };
            assert!(
                matches!(error, EngineError::Conflict(_)),
                "unexpected for a {state} container: {error:?}"
            );
        }
    }

    // ---- what the scenarios are built from ----------------------------------

    /// The specification every scenario starts from: the contract's image and
    /// network, this suite's own label, no environment and no binds.
    ///
    /// `user` is `0:0` rather than the session's `1000:1000`, for the reason
    /// `common::engine::test_spec` gives: the image the suite runs may have no
    /// account at uid 1000, and no scenario here asserts anything about
    /// ownership.
    fn spec(&self, scenario: &str) -> ContainerSpec {
        ContainerSpec {
            image: self.image.clone(),
            name: unique_name(scenario),
            labels: BTreeMap::from([(LABEL_TEST.to_string(), run_id().to_string())]),
            user: "0:0".to_string(),
            working_dir: "/".to_string(),
            cmd: TRAPPING_CMD
                .iter()
                .map(|part| (*part).to_string())
                .collect(),
            env: Vec::new(),
            secret_env: Vec::new(),
            binds: Vec::new(),
            network: self.env.network.clone(),
            extra_hosts: Vec::new(),
            runtime: None,
        }
    }

    /// Create one container and register it for cleanup.
    async fn create(&self, spec: &ContainerSpec) -> ContainerId {
        let id =
            self.engine.create(spec).await.unwrap_or_else(|error| {
                panic!("the container {} was not created: {error}", spec.name)
            });
        (self.env.container_created)(&id);
        id
    }

    /// A created container and the spec it was created from.
    async fn created(&self, scenario: &str) -> (ContainerSpec, ContainerId) {
        let spec = self.spec(scenario);
        let id = self.create(&spec).await;
        (spec, id)
    }

    /// A started container.
    async fn running(&self, scenario: &str) -> (ContainerSpec, ContainerId) {
        let (spec, id) = self.created(scenario).await;
        self.engine.start(&id).await.expect("the container starts");
        (spec, id)
    }

    /// A container that has run and exited, through the only route the trait
    /// offers: a stop, whose `SIGTERM` the command's trap turns into
    /// [`TERM_EXIT_CODE`].
    async fn exited(&self, scenario: &str) -> (ContainerSpec, ContainerId) {
        let (spec, id) = self.running(scenario).await;

        self.engine
            .stop(&id, STOP_GRACE_SECS)
            .await
            .expect("a running container stops");
        self.wait_within(&id).await;

        (spec, id)
    }

    /// Wait for a container to exit, inside a bound: an adapter that parks
    /// where the contract says it answers fails the scenario rather than
    /// hanging the run.
    async fn wait_within(&self, id: &ContainerId) -> ExitStatus {
        tokio::time::timeout(WAIT_TIMEOUT, self.engine.wait(id))
            .await
            .unwrap_or_else(|_| panic!("container {id} did not exit within {WAIT_TIMEOUT:?}"))
            .expect("the wait itself succeeds")
    }

    /// An id no engine has: a name in this suite's own shape that was never
    /// created.
    fn missing_id(&self) -> ContainerId {
        ContainerId(unique_name("contract-missing"))
    }

    /// One operation's refusal is [`EngineError::NotFound`].
    fn assert_not_found(&self, error: Option<EngineError>, operation: &str) {
        let Some(error) = error else {
            panic!("{operation} on a container the engine does not have succeeded");
        };
        assert!(
            matches!(error, EngineError::NotFound(_)),
            "unexpected for {operation}: {error:?}"
        );
    }
}
