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
//!
//! **The terminal.** The four `terminal_*` scenarios drive
//! `ws::terminal::Terminal` — the socket-independent half of the terminal
//! attachment — against a real container, so the path a keystroke takes is
//! proven on both engines and not only against `MockEngine` (`SPEC.md`,
//! "WebSocket: session stream"; the `exec + resize` row of the engine adapter
//! table). Three of them run on [`TEST_IMAGE`] and need nothing but
//! `DOCKER_HOST`, so the Engine CI workflow runs them as it stands.
//! `terminal_real_session_image_bash_as_agent` is the one that asserts the
//! literal `/bin/bash -l` as `agent`, and it needs a real session image: it
//! takes the tag from `MARS_STUB_IMAGE` (`README.md`, "Development") and
//! prints a line and passes when that is unset. The Engine CI workflow builds
//! the stub image before the engine step and passes its tag in, so in CI the
//! scenario runs on both the Podman and the Docker leg.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bollard::query_parameters::InspectContainerOptions;
use common::engine::{
    PULL_TEST_IMAGE, TEST_IMAGE, WAIT_TIMEOUT, absolute, await_terminal_closed,
    collect_terminal_output, connect_or_skip, ensure_test_image, plain_text, probe_lock,
    raw_docker, remove_image_if_present, rw_bind, test_spec, try_collect_terminal_output, uid_of,
    unique_name, with_cleanup, writable_tempdir,
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
    ContainerEngine, ContainerId, ContainerSpec, EngineError, EngineKind, ExecSession, ExitStatus,
    bootstrap_engine,
};
use mars_orchestrator::prelude::Config;
use mars_orchestrator::ws::terminal::{
    NO_EXIT_CODE, TERMINAL_CMD, TERMINAL_USER, Terminal, TerminalOutput,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

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

/// How long any terminal wait in this file may take.
///
/// [`WAIT_TIMEOUT`]'s thirty seconds rather than the ten a terminal answers in
/// when it is the only thing running: a full `cargo test` puts every test
/// binary on one engine at once, and a shell that has not printed its prompt
/// in ten seconds under that load is a busy machine, not a broken terminal.
/// The point of the bound is that a wedged wait fails instead of hanging the
/// run, and thirty seconds keeps that.
const TERMINAL_PATIENCE: Duration = WAIT_TIMEOUT;

/// How long closing a terminal whose shell is idle at its prompt may take: the
/// shell exits on the end of input the adapter writes, which is milliseconds,
/// and anything beyond this is one of the adapter's fallbacks having been
/// needed (`orchestrator/src/engine/streams.rs`).
const TERMINAL_CLOSE_PATIENCE: Duration = Duration::from_secs(3);

/// The output channel a socket would give a [`Terminal`]: deep enough that a
/// chatty login shell never makes the owner task wait on the scenario.
const TERMINAL_BUFFER: usize = 64;

/// What the fallback user is, when a login shell will not start as
/// [`TERMINAL_USER`] on one engine (the file header's uid note).
const FALLBACK_USER: &str = "0:0";

/// The stub session image's tag, from `MARS_STUB_IMAGE`.
///
/// The same variable `tests/session_e2e.rs` reads (`README.md`,
/// "Development"), and deliberately not a second one for the same image: the
/// Engine CI workflow already builds a stub image per engine and names it
/// there. Unset means the one scenario that needs a real session image skips
/// itself, which is what it does in CI today — the workflow builds the stub
/// after the engine step, so wiring the tag into that step is the CI follow-up
/// the task names, not a change here.
fn session_image() -> Option<String> {
    std::env::var("MARS_STUB_IMAGE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A throwaway container to attach a terminal to: [`TEST_IMAGE`] holding
/// itself open, with a `HOME` the login shell can use.
///
/// Alpine has no account at uid 1000 and therefore no home directory for one,
/// and `sh -l` started without a usable `HOME` prints a warning that lands in
/// the middle of the output the scenarios search. An exec inherits the
/// container's environment, so setting it here is what the exec sees.
fn terminal_spec(name: &str) -> ContainerSpec {
    let mut spec = test_spec(name, &["sleep", "300"]);
    spec.env = vec![("HOME".to_string(), "/tmp".to_string())];
    spec
}

/// Open a [`Terminal`] as [`TERMINAL_USER`], falling back to
/// [`FALLBACK_USER`] when no login shell comes up as uid 1000 on this engine.
///
/// The fallback is the file header's uid story again: a GitHub-hosted Docker
/// runner is uid 1001 and the test image has no account at 1000 at all. The
/// numeric user needs no passwd lookup, so the exec itself is expected to work
/// on both engines — but if it does not, the difference is printed and the
/// scenario carries on proving the terminal contract rather than failing on
/// the one thing that is the runner's.
///
/// The returned string is the user the terminal actually runs as, which is
/// what `terminal_adapter_login_shell_as_uid_1000` asserts `id -u` against.
async fn open_terminal_or_fall_back(
    engine: &BollardEngine,
    id: &ContainerId,
    cols: u16,
    rows: u16,
) -> (Terminal, mpsc::Receiver<TerminalOutput>, String) {
    let cmd: Vec<String> = ["/bin/sh", "-l"]
        .iter()
        .map(|part| (*part).to_string())
        .collect();

    for user in [TERMINAL_USER, FALLBACK_USER] {
        let (tx, mut rx) = mpsc::channel(TERMINAL_BUFFER);

        let terminal = match Terminal::open(engine, id, &cmd, user, cols, rows, tx).await {
            Ok(terminal) => terminal,
            Err(error) => {
                eprintln!("the terminal exec was refused as {user} on this engine: {error}");
                continue;
            }
        };

        // Two things at once. `stty -echo` stops the PTY repeating each
        // command back, so what the scenarios search is the shell's answer and
        // not the question; the marker proves a shell is really running behind
        // the PTY. The marker is written in two quoted halves so that the
        // *echo* of this line — which the shell still makes, the `stty` not
        // having run yet — does not itself contain the string being waited
        // for.
        terminal
            .write(b"stty -echo; echo mars-terminal'-'ready\n")
            .await
            .expect("the terminal takes input");

        match try_collect_terminal_output(&mut rx, "mars-terminal-ready", TERMINAL_PATIENCE).await {
            Ok(_) => return (terminal, rx, user.to_string()),
            Err(reason) => {
                eprintln!("no login shell came up as {user} on this engine: {reason}");
                terminal.close().await;
            }
        }
    }

    panic!("no login shell came up as {TERMINAL_USER} or {FALLBACK_USER}");
}

/// The terminal the browser gets, against a real engine: the login-shell exec
/// as uid 1000, the size the client asked for, and a clean exit code when the
/// shell leaves (`SPEC.md`, "WebSocket: session stream"; `ARCHITECTURE.md`,
/// "Engine adapter", the `exec + resize` row).
///
/// `exec_pty_echo_and_resize` above proves the adapter's own call. This proves
/// the thing above it — `ws::terminal::Terminal`, the socket-independent owner
/// task — over the same engine, which is the path a keystroke really takes.
#[tokio::test]
async fn terminal_adapter_login_shell_as_uid_1000() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = terminal_spec(&unique_name("terminal-login"));
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");

        let (terminal, mut rx, user) = open_terminal_or_fall_back(engine, &id, 100, 40).await;

        // The uid is labelled because the shell's prompt shares a chunk with
        // the answer often enough to matter — `<host>:/$ 1000` is one line,
        // not two — and a bare number is too easy to find in one anyway.
        terminal
            .write(b"echo uid=$(id -u); stty size\n")
            .await
            .expect("the terminal takes input");

        // `stty size` prints rows then columns: the window the client asked
        // for, carried through the adapter to the PTY.
        let seen = collect_terminal_output(&mut rx, "40 100", TERMINAL_PATIENCE).await;
        let text = plain_text(&seen);

        let expected_uid = user.split(':').next().expect("the user has a uid part");
        assert!(
            text.contains(&format!("uid={expected_uid}")),
            "the shell did not report uid {expected_uid}: {text:?}",
        );

        terminal
            .write(b"exit\n")
            .await
            .expect("the terminal takes input");

        assert_eq!(
            await_terminal_closed(&mut rx, TERMINAL_PATIENCE).await,
            0,
            "a shell that left by itself reports its own code",
        );
    })
    .await;
}

/// A resize reaches the live PTY, and a close while the shell is still running
/// answers inside its bound, ends the shell and leaves the container usable.
///
/// The resize is the half the mock cannot prove: only a real PTY reports the
/// size it was actually given. The close is the socket going away under a
/// shell that is still sitting at its prompt, which is what happens every time
/// a browser tab is shut.
///
/// That the shell is really gone is read here through `inspect_exec` on the
/// raw client, because no code path in the orchestrator can observe it: the
/// adapter hands back an exit code either way. It is the assertion that keeps
/// the two engines together, since the connection's half-close alone ends the
/// shell on Docker and not on rootless Podman (`ARCHITECTURE.md`, "Engine
/// adapter", the `exec + resize` row).
#[tokio::test]
async fn terminal_adapter_resize_and_close_while_alive() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = terminal_spec(&unique_name("terminal-resize"));
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");

        let (terminal, mut rx, _user) = open_terminal_or_fall_back(engine, &id, 80, 24).await;

        terminal
            .resize(120, 50)
            .await
            .expect("the terminal is open and takes a resize");
        terminal
            .write(b"stty size\n")
            .await
            .expect("the terminal takes input");
        collect_terminal_output(&mut rx, "50 120", TERMINAL_PATIENCE).await;

        // Whatever exec the container is carrying now is this terminal's; it
        // has to be read before the close, because a finished exec is not
        // listed on the container any more.
        let docker = raw_docker();
        let exec_ids = docker
            .inspect_container(&spec.name, None::<InspectContainerOptions>)
            .await
            .expect("the container inspects")
            .exec_ids
            .unwrap_or_default();
        assert!(
            !exec_ids.is_empty(),
            "the container reports no exec although a terminal is attached to it",
        );

        // The shell is alive and idle at its prompt: this is the socket
        // closing under it, not the shell leaving.
        let started = std::time::Instant::now();
        let code = tokio::time::timeout(TERMINAL_PATIENCE, terminal.close())
            .await
            .expect("the close is answered inside the bound");
        let took = started.elapsed();
        assert!(
            code > NO_EXIT_CODE,
            "the close reported no exit code at all: {code}",
        );
        // The shell takes the end of input the adapter writes: this is the
        // first drain, not the interrupt behind it and not the timeout behind
        // that (`orchestrator/src/engine/streams.rs`).
        assert!(
            took < TERMINAL_CLOSE_PATIENCE,
            "closing a shell at its prompt took {took:?}, which is the adapter's fallbacks \
             rather than the shell exiting on its end of input",
        );

        // Nothing of this terminal is left running in the container: the
        // adapter ends the shell through the PTY itself, so the engine that
        // does not pass the half-close on as an EOF ends it too.
        for exec_id in &exec_ids {
            let inspected = docker
                .inspect_exec(exec_id)
                .await
                .expect("the exec inspects");
            assert_ne!(
                inspected.running,
                Some(true),
                "the terminal exec {exec_id} is still running after the close \
                 (the terminal reported exit code {code})",
            );
        }

        let (second, mut second_rx, _user) = open_terminal_or_fall_back(engine, &id, 80, 24).await;
        second
            .write(b"exit\n")
            .await
            .expect("the terminal takes input");
        await_terminal_closed(&mut second_rx, TERMINAL_PATIENCE).await;
    })
    .await;
}

/// A terminal asked for on a container that has stopped is refused, and
/// refusing it leaves nothing running: no owner task, and therefore no output
/// channel still held open.
///
/// This is the client asking for a terminal on a session whose container went
/// away between the board and the click (`ARCHITECTURE.md`, "Engine adapter":
/// an operation on a container that is not running is a conflict).
#[tokio::test]
async fn terminal_adapter_on_stopped_container_is_conflict() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    let engine = &engine;
    ensure_test_image(engine).await;

    with_cleanup(engine, |cleanup| async move {
        let spec = test_spec(&unique_name("terminal-stopped"), &["true"]);
        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");
        wait_within(engine, &id, WAIT_TIMEOUT).await;

        let cmd: Vec<String> = ["/bin/sh", "-l"]
            .iter()
            .map(|part| (*part).to_string())
            .collect();
        let (tx, mut rx) = mpsc::channel(TERMINAL_BUFFER);

        let Some(error) = Terminal::open(engine, &id, &cmd, TERMINAL_USER, 80, 24, tx)
            .await
            .err()
        else {
            panic!("a terminal opened on a container that had stopped");
        };
        // A container that has exited is a conflict; one an engine has already
        // forgotten is a not-found. Both are refusals, and which one an engine
        // gives is not something the terminal depends on.
        assert!(
            matches!(error, EngineError::Conflict(_) | EngineError::NotFound(_)),
            "unexpected: {error:?}",
        );

        assert!(
            rx.recv().await.is_none(),
            "a refused open left an owner task holding the output channel",
        );
    })
    .await;
}

/// The real thing: `/bin/bash -l` as the `agent` user of a real session image
/// (`SPEC.md`, "WebSocket: session stream"; `ARCHITECTURE.md`, "Session
/// image", uid 1000).
///
/// The scenarios above run the same adapter against a plain test image, which
/// has neither `bash` nor an `agent` account, so they cannot assert the
/// command and the user `SPEC.md` names. This one does, and skips when there
/// is no session image to run it against.
#[tokio::test]
async fn terminal_real_session_image_bash_as_agent() {
    let Some(engine) = connect_or_skip().await else {
        return;
    };
    let Some(image) = session_image() else {
        eprintln!("MARS_STUB_IMAGE not set; skipping the real session image terminal test");
        return;
    };
    let engine = &engine;

    assert!(
        engine
            .image_exists(&image)
            .await
            .expect("the engine answers whether the session image is present"),
        "the session image {image} is not on this engine; build it with \
         `podman build -t {image} images/stub` (or point MARS_STUB_IMAGE at a tag you have)",
    );

    let image = &image;
    with_cleanup(engine, |cleanup| async move {
        let mut spec = test_spec(&unique_name("terminal-session"), &["sleep", "300"]);
        spec.image = image.clone();
        // The uid contract: the session container itself runs as 1000:1000,
        // and so does the terminal in it.
        spec.user = TERMINAL_USER.to_string();
        spec.working_dir = "/session/work".to_string();

        let id = engine
            .create(&spec)
            .await
            .expect("the container is created");
        cleanup.container(&id);
        engine.start(&id).await.expect("the container starts");

        let cmd: Vec<String> = TERMINAL_CMD
            .iter()
            .map(|part| (*part).to_string())
            .collect();
        let (tx, mut rx) = mpsc::channel(TERMINAL_BUFFER);
        let terminal = Terminal::open(engine, &id, &cmd, TERMINAL_USER, 100, 40, tx)
            .await
            .expect("the terminal opens on the session image");

        // As above: the marker is split so the pre-`stty` echo of this very
        // line does not satisfy the wait.
        terminal
            .write(b"stty -echo; echo mars-terminal'-'ready\n")
            .await
            .expect("the terminal takes input");
        collect_terminal_output(&mut rx, "mars-terminal-ready", TERMINAL_PATIENCE).await;

        // Both answers are labelled, because the session image's prompt is
        // `agent@<host>:/session/work$` and a bare `agent` would be satisfied
        // by the prompt that arrives before the command's own output — and
        // `bash` by that same prompt on an image whose `PS1` names the shell.
        // The labels are what make this an assertion about `$0` and `whoami`.
        terminal
            .write(b"echo shell=$0; echo user=$(whoami)\n")
            .await
            .expect("the terminal takes input");

        // `whoami` is the passwd lookup the numeric exec user does not need
        // but the image contract promises: uid 1000 is `agent`.
        let seen = collect_terminal_output(&mut rx, "user=agent", TERMINAL_PATIENCE).await;
        let text = plain_text(&seen);
        assert!(
            text.lines()
                .any(|line| line.starts_with("shell=") && line.contains("bash")),
            "the terminal is not a bash login shell: {text:?}",
        );

        terminal
            .write(b"exit\n")
            .await
            .expect("the terminal takes input");
        assert_eq!(
            await_terminal_closed(&mut rx, TERMINAL_PATIENCE).await,
            0,
            "a shell that left by itself reports its own code",
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
