---
id: wrpfa
title: Serialise container creation per engine host in the adapter
status: open
priority: P1
created: "2026-09-17T22:04:52.094559078Z"
updated: "2026-09-17T22:04:52.094559078Z"
tags:
  - orchestrator
  - engine
depends_on:
  - u6zkz
parent: s52qg
---

## Summary
Podman resolves `UsernsMode: keep-id:uid=1000,gid=1000` by calling the non-thread-safe `subid_get_uid_ranges` in shadow's `libsubid` through cgo, in the API service process, once per `create`. Two creates in flight corrupt each other's mapping: measured on rootless Podman 6.1.2, about one container in fourteen out of twenty concurrent creates comes out either with the degenerate mapping `1000:0:1` (no sub-uid range) or with the sub-uid range counted twice (`1001:1001:130072`), and the service itself segfaults in that call every few hundred creates, failing every request it is serving. A session container running as `1000:1000` is affected: it does not start, failing with `crun: write to /proc/sys/net/ipv4/ping_group_range (are all the IDs mapped in the user namespace?)` or `crun: write to uid_map: Operation not permitted`. The corruption is written into the container at `create`, so serialising creates is sufficient; `start`, `attach`, `wait` and the rest may stay concurrent. Two sessions launched at once on the same host therefore have to take turns creating their containers.

## Documents
- `ARCHITECTURE.md` "Engine adapter" (the `UsernsMode` row records the finding and the safe usage, "one `keep-id` create at a time per host"; add that the adapter enforces it)
- `README.md` "Podman setup" ("the orchestrator must create containers one at a time per host")
- An ADR in `docs/decisions/` only if the lock is placed in the launcher instead of the adapter, since that is the rejected alternative

## Scope
Hold a per-engine-host async mutex across the create call so that at most one container is ever being created per engine connection, releasing it before `start`. The natural place is `BollardEngine::create` (`orchestrator/src/engine/bollard.rs`), which already knows `self.kind` and is one instance per socket, rather than the launcher: it puts the constraint next to the operation it constrains and covers the startup probe as well. Taking the lock unconditionally is simpler than gating on `EngineKind::Podman` and costs Docker nothing measurable, but either is acceptable; decide and record which in the `UsernsMode` row. Do not serialise anything after `create`.

## Acceptance criteria
- [ ] `BollardEngine::create` never has two creates in flight on the same connection; the lock is released before `start`.
- [ ] A scenario in `orchestrator/tests/engine.rs` launches at least sixteen containers with the session `HostConfig` through the adapter concurrently and asserts every one starts; it fails on Podman 6.1.2 without the lock and passes with it.
- [ ] The engine suite's own `SERIAL` mutex in `tests/common/engine.rs::with_cleanup` is removed, since the adapter now guarantees what the mutex stood in for.
- [ ] `tests/engine.rs` passes ten consecutive runs on rootless Podman locally.
- [ ] `ARCHITECTURE.md` `UsernsMode` row states that the adapter enforces the one-create-at-a-time rule.

## Testing
- `for i in $(seq 10); do cargo test --features integration-tests --test engine; done` with `DOCKER_HOST` at the Podman socket.

## Reference
Bears u6zkz for the reproduction and the upstream stack trace.