//! The live engine suite: [`BollardEngine`] against whatever engine
//! `DOCKER_HOST` names.
//!
//! `CLAUDE.md`, "Testing expectations": these run only when `DOCKER_HOST` is
//! set, and CI runs them on both Podman and Docker. Every scenario begins with
//! [`connect_or_skip`], which prints a line and returns when the variable is
//! unset, so the suite passes — skipped — in a run that has no engine at all.
//!
//! **The contract comes first.** `engine_contract` runs the conformance suite
//! of `common::engine_contract` against the adapter: every line of the
//! normalised semantics of `ARCHITECTURE.md`, "Engine adapter", including the
//! lifecycle ordering, the label listing and a `SIGINT` reaching a
//! handler-installing PID 1 without `Init: true`. `tests/engine_mock.rs` runs
//! the same suite against `MockEngine` with no engine at all, so a rule only
//! one of the two satisfies is a failing test rather than a difference nobody
//! notices.
//!
//! What is left here is what the trait cannot express: the image pull, nested
//! bind mounts, `UsernsMode: keep-id:uid=1000,gid=1000` through Podman's
//! compatibility API, many creates at once, a real PTY, a real stdin delivery
//! into a bind mount, and [`bootstrap_engine`] end to end at the bottom, the
//! startup probe included — the same sequence the binary runs between its
//! migrations and its listeners. `keep-id` and the `SIGINT` were open questions; their answers
//! are recorded in `ARCHITECTURE.md`, "Engine adapter" and "Session image".
//!
//! **The uid contract, and why most scenarios ignore it.** The session
//! specification runs containers as `1000:1000` and expects the files they
//! write under `DATA_DIR` to come back owned by the orchestrator
//! (`ARCHITECTURE.md`, "Session container specification", Uid contract). That
//! holds on rootless Podman through `keep-id` and on a Docker host whose own
//! user is uid 1000. A GitHub-hosted Docker runner executes as uid 1001, so
//! there the contract does not hold and the startup probe is *expected* to
//! fail: `bootstrap_engine_end_to_end`, which runs the probe as part of the
//! bootstrap, asserts the failure branch there and the success branch
//! everywhere else. The scenarios that only need a file written — the stdin
//! attach and the nested binds — run as `0:0` into a world-writable temporary directory and assert
//! nothing about ownership, so they hold on every engine and every runner.
//! Only `userns_keep_id_accepted` and `bootstrap_engine_end_to_end` assert a
//! uid.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::engine::{
    PULL_TEST_IMAGE, TEST_IMAGE, WAIT_TIMEOUT, absolute, connect_or_skip, ensure_test_image,
    probe_lock, remove_image_if_present, rw_bind, test_spec, uid_of, unique_name, with_cleanup,
    writable_tempdir,
};
use common::engine_contract::{ContractEnv, assert_engine_contract};
use futures_util::future::join_all;
use mars_orchestrator::engine::bollard::BollardEngine;
// The three `spec` items each belong to a scenario that cannot be asserted
// through the trait: `order_binds` is the ordering `nested_bind_mounts_*` puts
// a deliberately wrong list through, `to_bollard` is where
// `userns_keep_id_accepted` reads the `HostConfig` field the trait does not
// expose, and `LABEL_PROBE` is the label the bootstrap scenario asserts no
// container is left carrying.
use mars_orchestrator::engine::spec::{LABEL_PROBE, order_binds, to_bollard};
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, EngineError, EngineKind, ExecSession, ExitStatus,
    bootstrap_engine,
};
use mars_orchestrator::prelude::Config;
use tokio::io::AsyncWriteExt;

/// The uid a session container runs as, and therefore the uid the probe
/// expects its file to come back owned by on a Docker host.
const SESSION_UID: u32 = 1000;

/// How many containers `concurrent_session_creates_all_start` launches at once.
///
/// Twenty is the number the reproduction used (Bears u6zkz), where about one in
/// fourteen came out with a broken mapping: enough that an unserialised run
/// fails nearly every time.
const CONCURRENT_CREATES: usize = 20;

/// Wait for a container to exit, inside a bound.
///
/// Every `wait` in the suite goes through this: a container that never exits
/// has to fail the scenario rather than hang the run.
async fn wait_within(engine: &BollardEngine, id: &ContainerId, budget: Duration) -> ExitStatus {
    tokio::time::timeout(budget, engine.wait(id))
        .await
        .unwrap_or_else(|_| panic!("container {id} did not exit within {budget:?}"))
        .expect("the wait itself succeeds")
}

/// Read from a PTY until its output contains `needle`, or give up.
///
/// The first bytes of an exec arrive in as many chunks as the engine feels
/// like, so a single read is not enough to see a whole line.
async fn read_until(exec: &mut Box<dyn ExecSession>, needle: &str) -> String {
    let budget = Duration::from_secs(20);

    tokio::time::timeout(budget, async {
        let mut seen = String::new();
        loop {
            match exec.read().await.expect("the pty reads") {
                Some(chunk) => seen.push_str(&String::from_utf8_lossy(&chunk)),
                None => panic!("the pty ended before {needle:?} arrived; saw: {seen:?}"),
            }
            if seen.contains(needle) {
                return seen;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{needle:?} did not arrive within {budget:?}"))
}

/// The conformance suite against [`BollardEngine`]: every line of the
/// normalised semantics of `ARCHITECTURE.md`, "Engine adapter", in the one
/// scenario `tests/engine_mock.rs` also runs against `MockEngine`.
///
/// Everything the suite asserts goes through `Arc<dyn ContainerEngine>`, so
/// what remains in this file is what cannot be asserted through the trait at
/// all: the pull, nested binds, `keep-id`, concurrent creates, a real PTY, a
/// real stdin delivery and the startup probe.
///
/// The two networks are this scenario's own and are registered for cleanup
/// before they are created; the suite registers every container it creates
/// through the same cleanup, so a scenario that fails halfway still leaves
/// nothing behind.
#[tokio::test]
async fn engine_contract() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let internal = unique_name("contract-int");
        let egress = unique_name("contract-egress");
        cleanup.network(&internal);
        cleanup.network(&egress);

        engine
            .ensure_network(&internal, true)
            .await
            .expect("the internal network is created");
        engine
            .ensure_network(&egress, false)
            .await
            .expect("the egress network is created");

        let env = ContractEnv {
            network: internal,
            second_network: egress,
            container_created: {
                let cleanup = Arc::clone(&cleanup);
                Box::new(move |id| cleanup.container(id))
            },
            network_created: {
                let cleanup = Arc::clone(&cleanup);
                Box::new(move |name| cleanup.network(name))
            },
        };

        // The trait object the suite is written against. The adapter is a cheap
        // handle around a shared transport, so the clone opens no second
        // connection.
        let under_test: Arc<dyn ContainerEngine> = Arc::new(engine.clone());
        assert_engine_contract(under_test, TEST_IMAGE, env).await;
    })
    .await;
}

/// The image is absent, `image_exists` says so, the pull fetches it and it is
/// there afterwards (`ARCHITECTURE.md`, "Engine adapter", image pull).
///
/// Its own tag, so removing the image cannot race a scenario creating a
/// container from [`TEST_IMAGE`].
#[tokio::test]
async fn pull_absent_image() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;

    remove_image_if_present(PULL_TEST_IMAGE).await;
    assert!(
        !engine
            .image_exists(PULL_TEST_IMAGE)
            .await
            .expect("the engine answers"),
        "the image was still present after being removed"
    );

    engine
        .pull_image(PULL_TEST_IMAGE)
        .await
        .expect("the image pulls");

    assert!(
        engine
            .image_exists(PULL_TEST_IMAGE)
            .await
            .expect("the engine answers"),
        "the image is absent after a successful pull"
    );
}

/// What a session image's entrypoint does for stdin, for a test command: make
/// the FIFO and hold it open read-write as stdin, so no writer going away is an
/// EOF (`ARCHITECTURE.md`, "Session image"; ADR 0034). The loop echoes each line
/// into the bind mount and ends on a sentinel, because EOF never comes.
fn fifo_echo_script() -> String {
    format!(
        r#"mkfifo {fifo}; exec 0<>{fifo}; while read line; do [ "$line" = END ] && exit 0; echo "$line" >> /mnt/out/echo.txt; done"#,
        fifo = mars_orchestrator::engine::STDIN_FIFO,
    )
}

/// The attach carries stdin and the container receives it, across two writes
/// separated in time: the session owner's whole use of the socket
/// (`ARCHITECTURE.md`, "Engine adapter", the stdin row).
#[tokio::test]
async fn attach_stdin_delivers_bytes() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let dir = writable_tempdir();
        let script = fifo_echo_script();
        let mut spec = test_spec(&unique_name("attach"), &["sh", "-c", &script]);
        spec.binds = vec![rw_bind(&absolute(dir.path()), "/mnt/out")];

        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");

        let mut stdin = engine.attach_stdin(&id).await.expect("stdin attaches");
        stdin.write_all(b"hello\n").await.expect("the first write");
        stdin.flush().await.expect("the first flush");
        tokio::time::sleep(Duration::from_secs(2)).await;
        stdin.write_all(b"world\n").await.expect("the second write");
        stdin.flush().await.expect("the second flush");
        stdin.write_all(b"END\n").await.expect("the sentinel write");
        stdin.flush().await.expect("the sentinel flush");

        assert_eq!(wait_within(engine, &id, WAIT_TIMEOUT).await.code, 0);

        let written = std::fs::read_to_string(dir.path().join("echo.txt"))
            .expect("the container wrote the file");
        assert_eq!(written, "hello\nworld\n");
    })
    .await;
}

/// An orchestrator restart, as the engine sees it: the attachment is dropped
/// and another one is made. The container's process must not notice — a
/// rootless Podman turns a closed *container* attach into EOF on stdin, which
/// the CLI exits on, and that is why the attachment is an exec relaying into a
/// FIFO the process holds open itself (ADR 0034; `ARCHITECTURE.md`, "Restart
/// procedure"). Both writes reach the one process, in order.
#[tokio::test]
async fn a_dropped_stdin_attachment_leaves_the_process_running_and_reattachable() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let dir = writable_tempdir();
        let script = fifo_echo_script();
        let mut spec = test_spec(&unique_name("reattach"), &["sh", "-c", &script]);
        spec.binds = vec![rw_bind(&absolute(dir.path()), "/mnt/out")];

        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");

        let mut first = engine.attach_stdin(&id).await.expect("stdin attaches");
        first.write_all(b"before\n").await.expect("the first write");
        first.flush().await.expect("the first flush");
        drop(first);

        // Long enough for an EOF to have ended the loop, had one arrived: the
        // defect this guards against exited within about 100 ms.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let info = engine.inspect(&id).await.expect("the container inspects");
        assert!(
            info.state.is_running(),
            "dropping the attachment ended the process: {:?}",
            info.state
        );

        let mut second = engine.attach_stdin(&id).await.expect("stdin reattaches");
        second
            .write_all(b"after\n")
            .await
            .expect("the second write");
        second
            .write_all(b"END\n")
            .await
            .expect("the sentinel write");
        second.flush().await.expect("the second flush");

        assert_eq!(wait_within(engine, &id, WAIT_TIMEOUT).await.code, 0);

        let written = std::fs::read_to_string(dir.path().join("echo.txt"))
            .expect("the container wrote the file");
        assert_eq!(
            written, "before\nafter\n",
            "one process read both attachments"
        );
    })
    .await;
}

/// A container with no FIFO — an image whose entrypoint does not honour the
/// contract — refuses the attach, instead of handing back a writer whose first
/// message would be lost.
#[tokio::test]
async fn attach_stdin_without_the_fifo_is_refused() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(&unique_name("no-fifo"), &["sleep", "300"]);
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");

        let Some(error) = engine.attach_stdin(&id).await.err() else {
            panic!("an attach with no FIFO to relay into was accepted");
        };
        assert!(
            matches!(error, EngineError::Unsupported(_)),
            "unexpected: {error:?}"
        );
    })
    .await;
}

/// The terminal view: an exec with a PTY at the size the client asked for,
/// bytes in both directions, a resize the engine accepts, and an exit code on
/// close (`SPEC.md`, "WebSocket: session stream").
#[tokio::test]
async fn exec_pty_echo_and_resize() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(&unique_name("exec-pty"), &["sleep", "300"]);
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");

        // The second waits for the resize. `exec_pty` starts the exec and then
        // resizes it, because there is no PTY to resize before the process has
        // one, and that resize is a request of its own: a shell that printed
        // its size the instant it started would print the engine's default and
        // race the request rather than observe it.
        let cmd: Vec<String> = ["sh", "-c", "sleep 1; stty size; cat"]
            .iter()
            .map(|part| (*part).to_string())
            .collect();
        let mut exec = engine
            .exec_pty(&id, &cmd, "0:0", 100, 40)
            .await
            .expect("the exec starts");

        // `stty size` prints rows then columns, which is the exec's own PTY at
        // the size `exec_pty` set after starting it.
        read_until(&mut exec, "40 100").await;

        exec.write(b"ping\n").await.expect("the pty takes input");
        read_until(&mut exec, "ping").await;

        exec.resize(120, 50).await.expect("the pty resizes");

        // Each exec has a PTY of its own, so the resize above is not visible
        // here: this one reports the size it was started with.
        let second_cmd: Vec<String> = ["sh", "-c", "sleep 1; stty size"]
            .iter()
            .map(|part| (*part).to_string())
            .collect();
        let mut second = engine
            .exec_pty(&id, &second_cmd, "0:0", 120, 50)
            .await
            .expect("the second exec starts");
        read_until(&mut second, "50 120").await;
        second.close().await.expect("the second exec closes");

        // EOF to `cat`, so the shell ends; the engine answers with its code,
        // or with -1 when it has none (`SPEC.md`: `terminal_closed` carries an
        // `exit_code` either way).
        let code = exec.close().await.expect("the exec closes");
        assert!(code >= -1, "unexpected exit code: {code}");
    })
    .await;
}

/// Nested bind mounts, parents before children: a shared directory mounted
/// inside the work tree is visible and is not hidden by the clone's own mount
/// (`ARCHITECTURE.md`, "Storage", Shared directories; "Engine adapter", nested
/// bind mounts).
///
/// The list is built in the wrong order on purpose and put through
/// [`order_binds`], which is the function the launcher relies on.
#[tokio::test]
async fn nested_bind_mounts_parent_before_child() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let parent = writable_tempdir();
        let child = writable_tempdir();

        let mut binds = vec![
            rw_bind(&absolute(child.path()), "/work/target"),
            rw_bind(&absolute(parent.path()), "/work"),
        ];
        order_binds(&mut binds);
        assert_eq!(
            binds[0].container_target, "/work",
            "the parent is not ordered first"
        );

        let mut spec = test_spec(
            &unique_name("nested-binds"),
            &[
                "sh",
                "-c",
                "echo p > /work/p.txt && echo c > /work/target/c.txt",
            ],
        );
        spec.binds = binds;

        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");
        assert_eq!(wait_within(engine, &id, WAIT_TIMEOUT).await.code, 0);

        assert!(
            parent.path().join("p.txt").exists(),
            "the parent mount did not receive the file written to /work"
        );
        assert!(
            child.path().join("c.txt").exists(),
            "the child mount did not receive the file written to /work/target"
        );
        assert!(
            !parent.path().join("target").join("c.txt").exists(),
            "the child's file landed under the parent: the child mount was hidden"
        );
    })
    .await;
}

/// `ARCHITECTURE.md`, "Engine adapter", the `UsernsMode` row: the adapter
/// serialises `create` per engine host, so launching many sessions at once
/// cannot corrupt their uid mappings.
///
/// [`CONCURRENT_CREATES`] containers with the session's own `1000:1000` and the
/// `HostConfig` [`to_bollard`] builds for it are created and started as
/// concurrently as the runtime will do it, and every one of them has to start
/// and exit 0. Without the adapter's lock this fails on rootless Podman 6.1.2:
/// Podman resolves `keep-id` through the non-thread-safe `libsubid`, about one
/// container in fourteen comes out with a broken mapping and `start` then
/// refuses it with `write to uid_map: Operation not permitted` (Bears u6zkz).
/// On Docker there is no user namespace to get wrong and this is simply twenty
/// containers.
///
/// `multi_thread`, because the point is many creates genuinely in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_session_creates_all_start() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let launches = (0..CONCURRENT_CREATES).map(|index| {
            let cleanup = Arc::clone(&cleanup);
            async move {
                let mut spec = test_spec(
                    &unique_name(&format!("concurrent-{index}")),
                    &["sh", "-c", "id -u"],
                );
                // The uid contract's own user: the only one `keep-id` is
                // resolved for, and therefore the only one the race touches.
                spec.user = "1000:1000".to_string();

                let id = engine
                    .create(&spec)
                    .await
                    .unwrap_or_else(|error| panic!("container {index} was not created: {error}"));
                cleanup.container(&id);

                engine.start(&id).await.unwrap_or_else(|error| {
                    panic!(
                        "container {index} did not start, which is the broken uid mapping this \
                         scenario is about: {error}"
                    )
                });

                assert_eq!(
                    wait_within(engine, &id, WAIT_TIMEOUT).await.code,
                    0,
                    "container {index} started and then failed"
                );
            }
        });

        join_all(launches).await;
    })
    .await;
}

/// `ARCHITECTURE.md`, "Engine adapter": `UsernsMode: keep-id:uid=1000,gid=1000`
/// is accepted through Podman's compatibility API, which this scenario is the
/// evidence for.
///
/// The adapter sets it from the engine kind and nothing else (ADR 0004), so
/// the scenario asserts both halves: that [`to_bollard`] put the field where
/// the engine will see it, and — on Podman — that a container created with it
/// as `1000:1000` writes files the test process owns. On Docker the field must
/// be absent and ownership is not asserted, because a Docker host maps no
/// namespace and its own uid may be anything.
#[tokio::test]
async fn userns_keep_id_accepted() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let dir = writable_tempdir();
        let mut spec = test_spec(
            &unique_name("userns"),
            &["sh", "-c", "id -u > /mnt/out/uid && touch /mnt/out/f"],
        );
        // The uid contract's own user, which is the whole point here.
        spec.user = "1000:1000".to_string();
        spec.binds = vec![rw_bind(&absolute(dir.path()), "/mnt/out")];

        let body = to_bollard(&spec, engine.kind());
        let userns = body
            .host_config
            .as_ref()
            .and_then(|host| host.userns_mode.clone());
        match engine.kind() {
            EngineKind::Podman => assert_eq!(
                userns.as_deref(),
                Some("keep-id:uid=1000,gid=1000"),
                "the adapter did not set keep-id on Podman"
            ),
            EngineKind::Docker => assert!(
                userns.is_none(),
                "the adapter set a user namespace on Docker: {userns:?}"
            ),
        }

        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");
        assert_eq!(
            wait_within(engine, &id, WAIT_TIMEOUT).await.code,
            0,
            "the container refused the user namespace or could not write"
        );

        let written =
            std::fs::read_to_string(dir.path().join("uid")).expect("the container wrote its uid");
        let reported: u32 = written
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("the container reported {written:?}: {error}"));
        assert_eq!(
            reported, SESSION_UID,
            "the container did not run as the session uid"
        );

        if engine.kind() == EngineKind::Podman {
            let file = dir.path().join("f");
            assert_eq!(
                uid_of(&file),
                uid_of(dir.path()),
                "keep-id was not honoured: the file the container wrote is owned by another uid"
            );
        }
    })
    .await;
}

/// [`bootstrap_engine`] end to end: exactly the sequence `main` runs between
/// the migrations and its listeners — connect, create both networks, probe —
/// driven from a `Config` (`ARCHITECTURE.md`, "Orchestrator internals"), and
/// with it the startup probe itself (`ARCHITECTURE.md`, "Engine adapter",
/// Startup probe): the probe container runs over a real data directory, the
/// file comes back owned by this process, and nothing — neither the directory
/// nor the container — is left behind.
///
/// On a host whose own uid is not 1000 and which maps no namespace — a
/// GitHub-hosted Docker runner — the probe is meant to fail, and this asserts
/// that failure instead. The exact message is asserted there: the probe makes
/// its session subdirectories world-writable, so the container's write
/// succeeds and the ownership check is what refuses.
#[tokio::test]
async fn bootstrap_engine_end_to_end() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    let _probe = probe_lock().lock().await;

    with_cleanup(engine, |cleanup| async move {
        let data = writable_tempdir();
        let data_dir = absolute(data.path());
        let internal = unique_name("boot-int");
        let egress = unique_name("boot-egress");
        // Registered before the bootstrap creates them, so a failure halfway
        // through still leaves nothing behind.
        cleanup.network(&internal);
        cleanup.network(&egress);

        let config = bootstrap_config(&data_dir, &internal, &egress);
        let outcome = bootstrap_engine(&config).await;

        let own_uid = uid_of(&data_dir);
        if probe_should_pass(engine.kind(), own_uid) {
            match outcome {
                Ok(bootstrapped) => assert_eq!(bootstrapped.kind(), engine.kind()),
                // The success type is not `Debug`, so the failure is matched
                // rather than unwrapped.
                Err(error) => panic!("the bootstrap failed: {error}"),
            }

            let leftovers: Vec<_> = std::fs::read_dir(data_dir.join("tmp"))
                .expect("the probe created DATA_DIR/tmp")
                .map(|entry| entry.expect("the entry reads").path())
                .collect();
            assert!(
                leftovers.is_empty(),
                "the probe left its directory behind: {leftovers:?}"
            );
        } else {
            assert_probe_failure(outcome.err(), own_uid);
        }

        // The networks the bootstrap was asked for exist whatever the probe
        // said, because it creates them before probing.
        for network in [&internal, &egress] {
            engine
                .ensure_network(network, true)
                .await
                .expect("the bootstrap created the network");
        }

        let probes = engine
            .list_by_label(LABEL_PROBE)
            .await
            .expect("the engine lists by label");
        assert!(
            probes.is_empty(),
            "the probe container survived the probe: {probes:?}"
        );
    })
    .await;
}

/// The configuration [`bootstrap_engine`] is driven with: the required
/// variables of `README.md`, "Configuration", with obviously fake values
/// (CLAUDE.md rule 3) and this scenario's own data directory, image and
/// networks.
fn bootstrap_config(data_dir: &std::path::Path, internal: &str, egress: &str) -> Config {
    let data_dir = data_dir.display().to_string();
    let docker_host = common::engine::docker_host().expect("DOCKER_HOST is set");

    let vars: HashMap<String, String> = [
        ("PUBLIC_URL", "https://mars.example.invalid"),
        ("JWT_SECRET", "not-a-real-signing-secret"),
        ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
        ("DOCKER_HOST", docker_host.as_str()),
        ("DATA_DIR_HOST", data_dir.as_str()),
        ("DATA_DIR", data_dir.as_str()),
        ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
        ("GIT_BOT_NAME", "Mars Bot"),
        ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
        ("SESSION_IMAGE_DEFAULT", TEST_IMAGE),
        ("SESSION_NETWORK_INTERNAL", internal),
        ("SESSION_NETWORK_EGRESS", egress),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();

    Config::from_vars(|name| vars.get(name).cloned()).expect("the fixture configuration loads")
}

/// Whether the uid contract holds here, and the probe therefore has to pass.
///
/// Podman maps the host user to 1000 with `keep-id`, so it always holds.
/// Docker maps nothing, so it holds only when the process is itself uid 1000
/// (the file header: a GitHub-hosted runner is 1001).
fn probe_should_pass(kind: EngineKind, own_uid: u32) -> bool {
    kind == EngineKind::Podman || own_uid == SESSION_UID
}

/// The other branch: the probe refused, naming both uids.
///
/// Only that one message is accepted. The probe's session subdirectories are
/// world-writable, so a container running as uid 1000 writes its file however
/// the host maps uids, and what is left to fail is the ownership check —
/// which is the failure an operator can act on.
fn assert_probe_failure(error: Option<EngineError>, own_uid: u32) {
    let Some(error) = error else {
        panic!("the probe passed although this host does not honour the uid contract");
    };
    let EngineError::Probe(message) = &error else {
        panic!("unexpected: {error:?}");
    };

    let expected_owner =
        format!("probe file is owned by uid {SESSION_UID}, orchestrator runs as uid {own_uid}");
    assert!(
        message.starts_with(&expected_owner),
        "the probe failed for an unexpected reason: {message}"
    );
}
