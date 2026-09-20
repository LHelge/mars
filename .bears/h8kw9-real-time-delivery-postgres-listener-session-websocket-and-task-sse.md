---
id: h8kw9
title: "Real-time delivery: Postgres listener, session WebSocket and task SSE"
type: epic
status: done
priority: P1
created: "2026-09-16T20:13:30.687434815Z"
updated: "2026-09-20T00:32:02.384052901Z"
tags:
  - orchestrator
  - realtime
depends_on:
  - s52qg
  - "5h3y4"
---

## Scope

The streaming surfaces the frontend subscribes to.

- `events/` fan-out: one shared Postgres `LISTEN` connection for `session_events`, `task_events` and `session_state`, forwarding to in-process broadcast channels keyed by session or project; notifications are wake signals only (ADRs 0005, 0028).
- `ws/`: `GET /ws/sessions/{id}?after=&token=`: subscribe before replay, replay `seq > after`, live reads on notification, 30-second safety read, `event`/`session`/`input_accepted`/`input_rejected`/`error` messages, `input` and `stop` forwarding through the registry, pings every 30 s and close after two missed pongs, re-authorization (account, `auth_version`, password gate) before each application message and at ping ticks with `authentication required` and close code 1008.
- Terminal multiplexing: `terminal_open` (running sessions only) creating an exec PTY running `/bin/bash -l` as `agent`, binary frames both ways, `terminal_resize`, `terminal_close`, `terminal_closed` with exit code, disposal on auth failure or socket close.
- `sse/`: `GET /api/projects/{pid}/tasks/stream?token=` with `Last-Event-ID` / `?after=` replay, `id`/`event: task`/`data` framing, `: keepalive` every 15 s, re-authorization at keepalive ticks, subscription established before the response opens.
- `?token=` acceptance validated exactly like the header for both endpoints.

## Documents

`SPEC.md` "WebSocket: session stream", "SSE: task stream", "Authentication" (stream rules); `ARCHITECTURE.md` "Event delivery", "User authentication and revocation"; `docs/data-model.md` "Notifications"; ADRs 0005, 0025, 0028.

## Acceptance criteria

- [ ] WebSocket tests: replay then live delivery with no gap or duplicate when an event commits during replay; input acceptance and rejection; revocation closes with 1008; ephemeral sessions reject input.
- [ ] SSE tests: replay from `Last-Event-ID`, live delivery, keepalive framing, revocation closes the stream.
- [ ] Terminal exec is exercised in the engine tests on both engines.

## Out of scope

Frontend hooks (frontend epics); tracker event production (Task tracker epic).