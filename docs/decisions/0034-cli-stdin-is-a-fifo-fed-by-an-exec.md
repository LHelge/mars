# 0034. The CLI's stdin is a FIFO it holds open itself, fed through an exec

Status: accepted.

## Context

The session owner wrote user messages to the CLI through an attach to the container's stdin (`OpenStdin: true`, `StdinOnce: false`). An orchestrator restart, graceful or not, closes that connection, and the two engines make different things of the close: Docker keeps the container's stdin open, while rootless Podman — the target engine (ADR 0004), observed on 4.9.3 and 6.1.2, through the compat API and with plain `podman attach` alike — passes it on to the container as EOF. The pinned CLI exits 0 on stdin EOF, as the stub does. Measured in `orchestrator/tests/session_e2e.rs` (Bears jt93h, gg4ch): the container exited within about 100 ms of the owners being dropped, before recovery listed the containers, so every running conversational session came back `parked` with reason "CLI exited" and whatever turn it was in was interrupted. Nothing was lost — transcript, offsets, checkout and `--resume` all survived (ADR 0003, ADR 0010) — but the restart procedure's promise that running sessions are re-adopted held on Docker only.

The CLI's stdin therefore has to stay open independently of any connection the orchestrator holds.

## Decision

The image's entrypoint makes a FIFO at `/tmp/mars-stdin`, inside the container's own filesystem, and `exec`s the CLI with its stdin opened **read-write** on it (`<>`). The CLI is still PID 1, and because the process itself holds a write end, the FIFO always has a writer while the process lives: no other writer going away can ever produce an EOF.

The engine adapter's `attach_stdin` no longer attaches to the container. It starts an exec, as the container's own user and without a TTY, that waits for the FIFO to exist, prints `ready`, and then `cat`s its stdin into the FIFO; the adapter hands the writer back once it has read `ready`. The exec's hijacked connection is the owner's writer, exactly as the attach's was, and its output half ending still means the container has gone. When the connection closes — an owner dropped, an orchestrator restarted or killed — the `cat` reaches its own EOF and exits, and the CLI notices nothing. The adopting owner calls `attach_stdin` again and its relay writes to the same process. The container is created with `OpenStdin: false`; its own stdin is unused.

The `ContainerEngine` trait is unchanged, so `MockEngine`, the owner, the launcher and recovery are as they were, and the single-writer rule holds: the relay is the owner's pipe, and a previous owner's relay has exited (or, on an engine that leaves it, has no input left to write). The exec runs as uid 1000 on a FIFO uid 1000 made, so the uid contract is untouched. The stub image ships the same entrypoint, byte for byte.

Rejected alternatives:

- **A FIFO on the session volume, written by the orchestrator directly**, with no engine connection at all. It is the least machinery, but a FIFO connects processes of one kernel, and the orchestrator and the container are not always on one: in development on macOS the engine runs in a VM and `DATA_DIR_HOST` is a directory shared into it (`ARCHITECTURE.md`, "Development on the host"). It would also have taken stdin out of the engine trait, and with it the mock every owner, launcher and recovery test observes input through.
- **A holder process in the image** that owns stdin and relays from a socket or a FIFO. A wrapper makes the CLI something other than PID 1 and has to forward `SIGINT` and `SIGTERM` faithfully, which the stop sequence depends on ("Stop semantics"); opening the FIFO read-write gives the CLI its own write end and needs no process to hold one. A socket would additionally need a client in the orchestrator and a route to the container that the FIFO-plus-exec path gets from the engine for free.
- **An upstream or compat-API fix.** None exists for the pinned Podman versions, `StdinOnce: false` is already what is asked for, and Mars cannot require an engine newer than the one its users have. Relying on Docker's behaviour was the defect.
- **Writing to `/proc/1/fd/0` from the relay** instead of a named path. It survives the name being unlinked, but before the entrypoint has run its redirect that descriptor is the container's original stdin, and a relay that raced it would lose input silently. The named FIFO can be waited for.

## Consequences

- A restart on either engine leaves running sessions running; adoption is a reattach to a live process. `tests/session_e2e.rs` asserts it (same container, still `running`, the fixture's second turn answers the next message, one `init`, one launch), `tests/engine.rs` asserts the mechanism (a dropped writer, a second writer, one reader), and the engine contract gains "dropping the writer leaves the process running".
- The session image contract gains the FIFO, and needs a POSIX `sh` and `cat` for the relay. An image whose entrypoint makes no FIFO fails the attach with `Unsupported` after the relay's ten-second wait, which fails the launch with that message rather than losing the first input.
- The CLI never sees EOF on stdin. Nothing in Mars relied on it: sessions are ended with `SIGINT` then `SIGTERM`. `images/smoke-test.sh` ends its conversational check with a signal for the same reason.
- An agent that unlinks `/tmp/mars-stdin` does not disturb its own running process, which keeps its open descriptor; only a later reattach fails, and recovery then parks the session as it does for any attach it cannot make — the behaviour every restart had before this decision.
- On an engine that does not end an exec's process when its client disconnects, one idle `cat` per restart would be left in the container until it exits. It holds no input and writes nothing.
