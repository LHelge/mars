---
id: naqhy
title: Container engine adapter
type: epic
status: done
priority: P1
created: "2026-09-16T20:12:18.569396170Z"
updated: "2026-09-17T20:37:00.094476184Z"
tags:
  - orchestrator
  - engine
depends_on:
  - sywed
---

## Scope

`engine/`: the `ContainerEngine` trait, its `bollard` implementation and the mock (ADR 0004).

- Trait operations from the `ARCHITECTURE.md` "Engine adapter" table: create, start, stop, kill with named signal, remove, attach stdin (TTY off), exec with PTY and resize, list by label, image pull, network connect, inspect/wait for exit.
- Engine detection through `/version` (Podman vs Docker) and the `UsernsMode: keep-id:uid=1000,gid=1000` rule applied only on Podman.
- Startup: create `SESSION_NETWORK_INTERNAL` (internal) and `SESSION_NETWORK_EGRESS` if missing; run the startup probe container from the default session image with a session-equivalent `HostConfig`, verify the written file is owned by the orchestrator's uid and writable, fail startup otherwise.
- A `ContainerSpec` builder that turns the "Session container specification" table (image, name, labels, user, workdir, env, stdin flags, binds in parent-before-child order, networks, extra hosts, `CapDrop ALL`, `no-new-privileges`, runtime) into engine calls and nothing else.
- Mock engine behind `integration-tests` with `as_any()`, recording created specs and simulating exit.
- `tests/engine.rs`: runs only when `DOCKER_HOST` is set; covers create/start/kill signal delivery to PID 1, attach stdin, exec PTY, label listing, nested bind mounts, second-network connect, on both Podman and Docker in CI (resolves open questions 7 and 8; write the answers back into `ARCHITECTURE.md` and delete the entries).

## Documents

`ARCHITECTURE.md` "Engine adapter", "Session container specification", "Uid contract", "Networks"; `README.md` "Podman setup"; ADR 0004; `docs/open-questions.md` items 7, 8.

## Acceptance criteria

- [ ] Engine tests pass on both Podman and Docker in CI.
- [ ] The probe fails startup with a clear log line when uid mapping is wrong.
- [ ] No `HostConfig` field outside the documented table is used.

## Out of scope

The launcher that decides what to run (Session lifecycle epic); the images themselves (Session images epic).