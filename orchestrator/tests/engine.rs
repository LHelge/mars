//! The live engine suite: [`BollardEngine`] against whatever engine
//! `DOCKER_HOST` names.
//!
//! `CLAUDE.md`, "Testing expectations": these run only when `DOCKER_HOST` is
//! set, and CI runs them on both Podman and Docker. Every scenario begins with
//! [`connect_or_skip`], which prints a line and returns when the variable is
//! unset, so the suite passes — skipped — in a run that has no engine at all.
//!
//! One scenario per row of the operation table in `ARCHITECTURE.md`, "Engine
//! adapter", plus the two behaviours the table records as verified rather than
//! merely available: `UsernsMode: keep-id:uid=1000,gid=1000` through Podman's
//! compatibility API (`docs/open-questions.md` item 8) and a `SIGINT` reaching
//! PID 1 without `Init: true` (item 7). The startup probe and
//! [`bootstrap_engine`] run end to end at the bottom, which is the same
//! sequence the binary runs between its migrations and its listeners.
//!
//! **The uid contract, and why most scenarios ignore it.** The session
//! specification runs containers as `1000:1000` and expects the files they
//! write under `DATA_DIR` to come back owned by the orchestrator
//! (`ARCHITECTURE.md`, "Session container specification", Uid contract). That
//! holds on rootless Podman through `keep-id` and on a Docker host whose own
//! user is uid 1000. A GitHub-hosted Docker runner executes as uid 1001, so
//! there the contract does not hold and the startup probe is *expected* to
//! fail: `startup_probe_end_to_end` and `bootstrap_engine_end_to_end` assert
//! the failure branch there and the success branch everywhere else. The
//! scenarios that only need a file written — the stdin attach and the nested
//! binds — run as `0:0` into a world-writable temporary directory and assert
//! nothing about ownership, so they hold on every engine and every runner.
//! Only `userns_keep_id_accepted` and the two probe scenarios assert a uid.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::engine::{
    LABEL_TEST, PULL_TEST_IMAGE, TEST_IMAGE, WAIT_TIMEOUT, absolute, connect_or_skip,
    ensure_test_image, probe_lock, remove_image_if_present, run_id, rw_bind, test_spec, uid_of,
    unique_name, with_cleanup, writable_tempdir,
};
use mars_orchestrator::engine::bollard::BollardEngine;
use mars_orchestrator::engine::probe::{ProbeInput, run_startup_probe};
use mars_orchestrator::engine::spec::{LABEL_PROBE, order_binds, to_bollard};
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerState, EngineError, EngineKind, ExecSession, ExitStatus,
    LABEL_SESSION_ID, Signal, bootstrap_engine,
};
use mars_orchestrator::prelude::Config;
use tokio::io::AsyncWriteExt;

/// The uid a session container runs as, and therefore the uid the probe
/// expects its file to come back owned by on a Docker host.
const SESSION_UID: u32 = 1000;

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

/// create → start → wait, and the exit code survives both the wait and a later
/// inspect.
#[tokio::test]
async fn create_start_wait_exit_code() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(&unique_name("exit-code"), &["sh", "-c", "exit 7"]);
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);

        engine.start(&id).await.expect("the container starts");
        let status = wait_within(engine, &id, WAIT_TIMEOUT).await;

        assert_eq!(status.code, 7, "the exit code the command chose");
        assert!(!status.oom_killed, "nothing was killed by the OOM killer");

        let info = engine.inspect(&id).await.expect("the container inspects");
        assert_eq!(info.state, ContainerState::Exited { code: 7 });
        assert_eq!(info.name, spec.name);
        assert_eq!(
            info.labels.get(LABEL_TEST).map(String::as_str),
            Some(run_id())
        );
    })
    .await;
}

/// `docs/open-questions.md` item 7: does a `SIGINT` sent with `kill` reach PID
/// 1 without `Init: true`?
///
/// The container's command *is* PID 1 — nothing sets `Init`, and the scenario
/// deliberately does not, because observing the default is the point. The
/// shell installs a handler for `INT` and exits 42 from it, so the exit code
/// is proof the signal was delivered and handled rather than the container
/// merely being torn down.
///
/// If this ever fails on an engine the answer is not to ignore the test: the
/// spec gains `Init: true` and `ARCHITECTURE.md`, "Session image" changes with
/// it, and this scenario then asserts the new contract.
///
/// The loop wakes once a second, so the exit can trail the kill by up to that;
/// ten seconds is a bound on the engine, not on the shell.
#[tokio::test]
async fn kill_sigint_reaches_pid1_without_init() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(
            &unique_name("sigint"),
            &[
                "sh",
                "-c",
                r#"trap "exit 42" INT; while true; do sleep 1; done"#,
            ],
        );
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);

        engine.start(&id).await.expect("the container starts");
        // The trap has to be installed before the signal arrives.
        tokio::time::sleep(Duration::from_secs(1)).await;

        engine
            .kill(&id, Signal::Sigint)
            .await
            .expect("the container takes the signal");

        let status = wait_within(engine, &id, Duration::from_secs(10)).await;
        assert_eq!(
            status.code, 42,
            "SIGINT did not reach PID 1: the container exited {} instead of running its INT trap",
            status.code
        );
    })
    .await;
}

/// The same for `SIGTERM`, with the exit code the stop sequence's hard stop
/// produces (`ARCHITECTURE.md`, "Stop semantics").
///
/// The trap is not decoration. A process that is PID 1 of a pid namespace
/// receives a signal from outside that namespace only if it has installed a
/// handler for it — `SIGKILL` and `SIGSTOP` excepted (`pid_namespaces(7)`) —
/// so a plain `while true; do sleep 1; done` ignores `SIGTERM` on every engine
/// and the container never exits, which would say nothing about the engine.
/// The Claude Code CLI installs its own handlers, which is why the session
/// command can be PID 1 without `Init: true` at all.
#[tokio::test]
async fn kill_sigterm_exit_code() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(
            &unique_name("sigterm"),
            &[
                "sh",
                "-c",
                r#"trap "exit 143" TERM; while true; do sleep 1; done"#,
            ],
        );
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);

        engine.start(&id).await.expect("the container starts");
        tokio::time::sleep(Duration::from_secs(1)).await;

        engine
            .kill(&id, Signal::Sigterm)
            .await
            .expect("the container takes the signal");

        let status = wait_within(engine, &id, Duration::from_secs(10)).await;
        assert_eq!(status.code, 143, "the SIGTERM exit code");
    })
    .await;
}

/// Signalling a container that has already exited is a conflict, which is what
/// the session owner reads as "it is already gone" rather than as a failure.
#[tokio::test]
async fn kill_on_exited_container_is_conflict() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(&unique_name("kill-exited"), &["sh", "-c", "exit 0"]);
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);

        engine.start(&id).await.expect("the container starts");
        assert_eq!(wait_within(engine, &id, WAIT_TIMEOUT).await.code, 0);

        let error = engine
            .kill(&id, Signal::Sigint)
            .await
            .expect_err("a container that has exited cannot be signalled");
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );
    })
    .await;
}

/// Removing a container that is not there is success: orphan cleanup and the
/// end of a session both want the container gone, and it is.
#[tokio::test]
async fn remove_missing_is_ok() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;

    engine
        .remove(&ContainerId("does-not-exist".to_string()), true)
        .await
        .expect("a container that is already gone is already removed");
}

/// The label filter recovery runs on: every container carrying the key,
/// running or not, and nothing else (`ARCHITECTURE.md`, "Engine adapter", list
/// with label filter).
#[tokio::test]
async fn list_by_label_filters() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        // The value is this scenario's own, so the two containers it expects
        // can be picked out of an engine that is also running other sessions.
        let session_id = uuid::Uuid::new_v4().to_string();

        let mut running = test_spec(&unique_name("labelled-running"), &["sleep", "60"]);
        running
            .labels
            .insert(LABEL_SESSION_ID.to_string(), session_id.clone());
        let mut exited = test_spec(&unique_name("labelled-exited"), &["sh", "-c", "exit 0"]);
        exited
            .labels
            .insert(LABEL_SESSION_ID.to_string(), session_id.clone());
        let unlabelled = test_spec(&unique_name("unlabelled"), &["sh", "-c", "exit 0"]);

        let running_id = engine.create(&running).await.expect("created");
        cleanup.container(&running_id);
        let exited_id = engine.create(&exited).await.expect("created");
        cleanup.container(&exited_id);
        let unlabelled_id = engine.create(&unlabelled).await.expect("created");
        cleanup.container(&unlabelled_id);

        engine
            .start(&running_id)
            .await
            .expect("the container starts");
        engine
            .start(&exited_id)
            .await
            .expect("the container starts");
        assert_eq!(wait_within(engine, &exited_id, WAIT_TIMEOUT).await.code, 0);

        let listed = engine
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

        let running_row = mine
            .iter()
            .find(|row| row.name == running.name)
            .expect("the running container is listed");
        assert!(
            running_row.running,
            "the running container is not reported running"
        );
        assert_eq!(running_row.id, running_id);

        let exited_row = mine
            .iter()
            .find(|row| row.name == exited.name)
            .expect("the exited container is listed");
        assert!(
            !exited_row.running,
            "the exited container is reported running"
        );

        assert!(
            !listed.iter().any(|row| row.name == unlabelled.name),
            "a container without the label was listed"
        );
    })
    .await;
}

/// The attach carries stdin and the container receives it, across two writes
/// separated in time: the session owner's whole use of the socket
/// (`ARCHITECTURE.md`, "Engine adapter", attach).
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
        let mut spec = test_spec(
            &unique_name("attach"),
            &["sh", "-c", "cat > /mnt/out/echo.txt"],
        );
        spec.open_stdin = true;
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
        // EOF, which is what makes `cat` exit.
        stdin.shutdown().await.expect("the writer shuts down");

        assert_eq!(wait_within(engine, &id, WAIT_TIMEOUT).await.code, 0);

        let written = std::fs::read_to_string(dir.path().join("echo.txt"))
            .expect("the container wrote the file");
        assert_eq!(written, "hello\nworld\n");
    })
    .await;
}

/// A write to a container that has exited fails rather than being silently
/// lost: the adapter treats the attach output half ending as the attachment
/// closing, because a rootless Podman accepts and discards writes to a
/// container that is gone (`ARCHITECTURE.md`, "Engine adapter", attach).
///
/// The container exits by itself a few seconds after starting, rather than
/// immediately: the attachment has to be in place *before* the exit, because
/// the exit is the event under test, and a command that is a bare `exit 0` is
/// gone within milliseconds of the start.
#[tokio::test]
async fn attach_stdin_write_after_exit_fails() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        // The command exits on its own, after long enough for the attach to be
        // in place: create, start, attach is the order the session owner uses,
        // and a container whose command is a bare `exit 0` would be gone
        // before the attach landed, which would test the race and not the
        // contract.
        let mut spec = test_spec(
            &unique_name("attach-exit"),
            &["sh", "-c", "sleep 5; exit 0"],
        );
        spec.open_stdin = true;

        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);

        engine.start(&id).await.expect("the container starts");
        let mut stdin = engine.attach_stdin(&id).await.expect("stdin attaches");
        assert_eq!(wait_within(engine, &id, WAIT_TIMEOUT).await.code, 0);

        // The attachment closes when the engine ends the stream, which trails
        // the exit by a little; a few seconds of polling is that gap, not a
        // wait for anything to happen.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
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

/// An exec on a container that is not running is a conflict on both engines,
/// although they number the refusal differently (`ARCHITECTURE.md`, "Engine
/// adapter", exec).
#[tokio::test]
async fn exec_on_stopped_container_is_conflict() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(&unique_name("exec-stopped"), &["sh", "-c", "exit 0"]);
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);

        engine.start(&id).await.expect("the container starts");
        assert_eq!(wait_within(engine, &id, WAIT_TIMEOUT).await.code, 0);

        let cmd = vec!["sh".to_string()];
        // The two engines number the refusal differently — Docker 409, Podman
        // 500 — and the adapter reports both as a conflict, which is what the
        // terminal answers a `terminal_open` with.
        let Some(error) = engine.exec_pty(&id, &cmd, "0:0", 80, 24).await.err() else {
            panic!("an exec on a container that is not running was accepted");
        };
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );
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

/// The launch order for networks: created on the internal one, connected to
/// the egress one, and only then started, so the container never runs with the
/// wrong set (`ARCHITECTURE.md`, "Networks").
#[tokio::test]
async fn second_network_connected_before_start() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let internal = unique_name("int");
        let egress = unique_name("egress");

        engine
            .ensure_network(&internal, true)
            .await
            .expect("the internal network is created");
        cleanup.network(&internal);
        engine
            .ensure_network(&egress, false)
            .await
            .expect("the egress network is created");
        cleanup.network(&egress);

        let mut spec = test_spec(&unique_name("two-networks"), &["sleep", "30"]);
        spec.network = internal.clone();

        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine
            .connect_network(&id, &egress)
            .await
            .expect("the egress network connects");
        engine.start(&id).await.expect("the container starts");

        let info = engine.inspect(&id).await.expect("the container inspects");
        assert!(
            info.networks.contains(&internal),
            "the internal network is missing: {:?}",
            info.networks
        );
        assert!(
            info.networks.contains(&egress),
            "the egress network is missing: {:?}",
            info.networks
        );
    })
    .await;
}

/// `ensure_network` is what startup calls on every boot: the second call finds
/// the network and succeeds without touching it.
#[tokio::test]
async fn ensure_network_is_idempotent() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;

    with_cleanup(engine, |cleanup| async move {
        let name = unique_name("idempotent");

        engine
            .ensure_network(&name, true)
            .await
            .expect("the network is created");
        cleanup.network(&name);

        engine
            .ensure_network(&name, true)
            .await
            .expect("an existing network is not an error");
    })
    .await;
}

/// `docs/open-questions.md` item 8: is `UsernsMode: keep-id:uid=1000,gid=1000`
/// accepted through Podman's compatibility API?
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

/// The startup probe end to end (`ARCHITECTURE.md`, "Engine adapter", Startup
/// probe): the container runs over a real data directory, the file comes back
/// owned by this process, and nothing is left behind.
///
/// On a host whose own uid is not 1000 and which maps no namespace — a
/// GitHub-hosted Docker runner — the probe is meant to fail, and this asserts
/// that failure instead. The message is checked loosely there because the
/// directory the orchestrator created is 0o755 and owned by the runner, so the
/// container may fail to write the file at all rather than write it as the
/// wrong uid; either way the verdict is `Probe` and startup stops.
#[tokio::test]
async fn startup_probe_end_to_end() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    // A reference, so the scenario body below can be an `async move` block
    // without moving the engine the cleanup still needs.
    let engine = &engine;
    ensure_test_image(engine).await;

    // Only one probe at a time: the assertion below is that no `mars.probe`
    // container survives, and a probe running beside this one would be one.
    let _probe = probe_lock().lock().await;

    with_cleanup(engine, |cleanup| async move {
        let data = writable_tempdir();
        let data_dir = absolute(data.path());
        let internal = unique_name("probe-int");
        let egress = unique_name("probe-egress");

        engine
            .ensure_network(&internal, true)
            .await
            .expect("the internal network is created");
        cleanup.network(&internal);
        engine
            .ensure_network(&egress, false)
            .await
            .expect("the egress network is created");
        cleanup.network(&egress);

        let outcome = run_startup_probe(
            engine,
            ProbeInput {
                image: TEST_IMAGE.to_string(),
                data_dir: data_dir.clone(),
                data_dir_host: data_dir.clone(),
                network_internal: internal.clone(),
                network_egress: egress.clone(),
                extra_hosts: Vec::new(),
            },
        )
        .await;

        let own_uid = uid_of(&data_dir);
        if probe_should_pass(engine.kind(), own_uid) {
            let report = outcome.expect("the probe passes");
            assert_eq!(report.engine_kind, engine.kind());
            assert_eq!(report.own_uid, own_uid);
            assert_eq!(
                report.file_uid, own_uid,
                "the probe file came back owned by another uid"
            );

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

/// [`bootstrap_engine`] end to end: exactly the sequence `main` runs between
/// the migrations and its listeners — connect, create both networks, probe —
/// driven from a `Config` (`ARCHITECTURE.md`, "Orchestrator internals").
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

/// The other branch: the probe refused, with a message naming what it found.
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
        message.starts_with(&expected_owner)
            || message.starts_with("probe file was not written")
            || message.starts_with("probe container exited with code"),
        "the probe failed for an unexpected reason: {message}"
    );
}
