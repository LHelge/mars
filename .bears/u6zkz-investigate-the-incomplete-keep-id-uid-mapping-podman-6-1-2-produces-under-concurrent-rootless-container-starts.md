---
id: u6zkz
title: Investigate the incomplete keep-id uid mapping Podman 6.1.2 produces under concurrent rootless container starts
status: done
priority: P1
created: "2026-09-17T19:49:57.463573029Z"
updated: "2026-09-17T22:04:34.024448070Z"
tags:
  - orchestrator
  - engine
depends_on:
  - "9ecwn"
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Running `orchestrator/tests/engine.rs` on rootless Podman 6.1.2 (this development machine, uid 1000, `/etc/subuid` `lhelge:100000:65536`), about one run in three fails one scenario at `start` with:

```
Api { status: 500, message: "container uses ID mappings ([]specs.LinuxIDMapping{specs.LinuxIDMapping{ContainerID:0x3e8, HostID:0x0, Size:0x1}}), but doesn't map UID 0" }
```

The failing scenario varies (`create_start_wait_exit_code`, `attach_stdin_write_after_exit_fails`, ...); every failing container was created with `user: "0:0"` and the session `HostConfig` (`UsernsMode: keep-id:uid=1000,gid=1000`) while other scenarios were creating and starting containers concurrently. The mapping Podman reports has only the single keep-id entry (container 1000 → the user) and no sub-uid range, so uid 0 is unmapped. A complete keep-id mapping has three entries. Serial runs and the same scenario in isolation pass every time.

Session containers run as `1000:1000`, which is mapped even in the degenerate mapping, so a session start would not fail with this message; but a container whose mapping lacks the sub-uid range sees every root-owned file in the image as `nobody`, which is not the documented contract ("Uid contract", `ARCHITECTURE.md`). Whether a session launched while another session is starting can receive the degenerate mapping is the question.

The engine suite mitigates this by running its scenarios one at a time (`tests/common/engine.rs`, the process-wide lock in the scenario wrapper); that mitigation is not a fix.

## Documents
- `ARCHITECTURE.md` "Engine adapter" (`UsernsMode` row; "the verification is recorded in this table"), "Session container specification" → "Uid contract"
- `README.md` "Podman setup"

## Acceptance criteria
- [ ] Reproduce outside the test suite (for example `N` concurrent `podman run --userns=keep-id:uid=1000,gid=1000 --user 0:0 alpine id` through the compat socket) and determine whether it is a Podman bug in the version used, a configuration issue on this machine, or a race in how the adapter creates containers.
- [ ] If it is Podman's: record the affected version and the safe usage (serialised starts, or a minimum version) in the `UsernsMode` row of the "Engine adapter" table and in `README.md` "Podman setup"; if the launcher has to serialise container creation per host, file that as a task in the Session lifecycle epic.
- [ ] If it is the adapter's: fix it in `engine/bollard.rs` or `engine/spec.rs` with a regression scenario in `tests/engine.rs`, and remove the serialisation from the suite.
- [ ] Either way, `tests/engine.rs` passes ten consecutive runs on rootless Podman locally.

## Testing
- `for i in $(seq 10); do cargo test --features integration-tests --test engine; done` with `DOCKER_HOST` at the Podman socket.