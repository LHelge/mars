---
id: "2acdq"
title: "Write session lifecycle E2E specs on the stub image: launch, transcript replay, message, stop, resume, end, failure and retry"
status: open
priority: P1
created: "2026-09-16T20:44:05.430449145Z"
updated: "2026-09-16T20:44:05.430449145Z"
tags:
  - frontend
  - sessions
  - tests
depends_on:
  - ku8up
parent: "6s8j7"
---

## Summary
Cover the "Sessions" feature paragraph on real containers running the stub image: launching a conversational session from the project page with a first message, watching the replayed fixture transcript render (text, tool calls with results, nested subagent, thinking, streamed deltas, permission denial, per-turn result with cost), sending a message mid-conversation and mid-turn ("Interject"), stopping (parked, "stopped" versus "killed"), resuming by sending a message, ending (done), and a session whose CLI exits non-zero (failed) followed by retry. The stub's environment knobs are passed through profile-declared project secrets.

## Documents
- `SPEC.md` "User-facing features", "Sessions" paragraph (launch from profile and base ref, optional first message, transcript with per-tool rendering and nested subagents, composer with mid-turn messages, stop button, metadata, parked looks like waiting, title defaults, cost and tokens).
- `SPEC.md` "Sessions" table (`POST /projects/{pid}/sessions` → 201 `state: creating`; `POST /sessions/{id}/input` → 202, relaunches if parked; `/stop` → 202 SIGINT then SIGTERM; `/end` → `Session` `done`; `/retry` `{message?}` from `failed` → `parked` then relaunched with a message; `Session` fields incl. `cli_session_id`, `cost_usd`, `input_tokens`, `output_tokens`, `error`).
- `ARCHITECTURE.md` "Session lifecycle" (state table; `creating → running` on init; `running → parked` on stop / CLI exit 0; `running → failed` on unrecoverable CLI error; `failed → parked` on retry), "Launch sequence" (queued inputs flushed after init), "Stop semantics" (`state_change` records the signal; `STOP_GRACE_SECS`), "Cost accounting", "Session image" (stub replays one turn per stdin line).
- `SPEC.md` "AgentEvent" (kinds `init`, `user_message`, `text_delta`, `text`, `thinking`, `tool_call`, `tool_result`, `permission_denied`, `subagent_start`/`subagent_end`, `result`, `state_change` with `signal`), "WebSocket: session stream" (replay from `after`, `input_accepted`), "Frontend" ("Session state", "Transcript rendering", "Composer").
- Stub contract from the images epic (`images/stub/claude`): fixture turns end at `result`; `MARS_STUB_LINE_DELAY_MS`, `MARS_STUB_EXIT_AFTER_TURNS`, `MARS_STUB_EXIT_CODE`, `MARS_STUB_FIXTURE`; after the fixture is exhausted every stdin line yields `Stub reply to: <text>`; SIGINT mid-turn yields a trailing `result` and exit 0; SIGTERM exits 143. Default fixture (`images/stub/fixtures/default.jsonl`): turn 1 text + `Read` tool + result (cost 0.0123); turn 2 thinking, `Task` subagent with nested `Grep`, `Edit`, `Bash`, text, result (0.0456), including a tool result above 8 KiB; turn 3 two `stream_event` deltas, `permission_denied`, text, result (0.0089).

## Acceptance criteria
- [ ] `frontend/tests/sessions.spec.ts`; every test creates a user, a bare repository and a ready project; `test.setTimeout(180_000)`.
- [ ] `launch with a first message and watch the transcript`: from the project sessions tab pick `default`, leave base ref at `main`, type first message `hello stub`, launch; the app navigates to `/sessions/:id`; header shows state `creating` then `running`, branch `session/<id>`, a container id and, after init, `cli_session_id`; title equals `hello stub`. Transcript shows the user message, an assistant text, a `Read` tool card with a collapsed summary that expands to its result, and a result row; header cost shows `$0.0123` (or the app's formatting of `0.0123`) and non-zero token counts.
- [ ] `second and third turns render subagent, edit diff, shell, deltas and denial`: send `next` → the `Task` tool card is a collapsible nested transcript containing the `Grep` child; the `Edit` card renders a diff with `old_string`/`new_string` content; the `Bash` card renders monospace; a tool result longer than 40 lines is collapsed with an expand control; cost header reads the sum `0.0579`. Send `more` → assistant text appears incrementally (assert an intermediate `streaming` render or at least the final text), a `permission_denied` system row names the denied tool, and the header cost is `0.0668`.
- [ ] `after the fixture is exhausted the stub echoes`: fourth message `echo me` → assistant text `Stub reply to: echo me`.
- [ ] `interject mid-turn`: project secret `MARS_STUB_LINE_DELAY_MS=400` declared on the default profile (`setProjectSecret` + `setProfileSecrets`); launch with a first message; while the turn is still streaming the composer button reads `Interject`; sending a message is accepted (`user_message` appears immediately, optimistic then confirmed) and, after the turn, the next turn plays.
- [ ] `stop parks the session and shows stopped`: with the same delay knob, click Stop during a turn; the session becomes `parked`, the container id is cleared, the transcript shows the `state_change` with `SIGINT` rendered as "stopped" (not "killed"); the composer is enabled (parked accepts input).
- [ ] `sending a message to a parked session relaunches it`: type and send; state `running` again; a new `init` with `resumed: true` is reflected (`cli_session_id` unchanged); the reply renders.
- [ ] `end moves to done and disables the composer`: click End; state `done`, `ended_at` set, composer disabled; `POST /sessions/{id}/input` from the API returns 409 or the 202-with-rejection the SPEC's state table implies for `done` (assert "not accepted"); the session branch `refs/sessions/<id>` exists in the mirror (`gitRevParse(mirrorPath(pid), "refs/sessions/<id>")`).
- [ ] `a CLI that exits non-zero fails the session and retry parks it`: secrets `MARS_STUB_EXIT_AFTER_TURNS=1`, `MARS_STUB_EXIT_CODE=1`; launch with a message; wait `failed`; header shows `error` text; remove the two secrets; click Retry with a message; state `parked` then `running`; the reply renders.
- [ ] `title defaults`: launch without `title` and with message `Line one\nLine two`: title is `Line one`; a message longer than 80 characters is truncated to 80.
- [ ] `parked idle session`: profile idle timeout set to the minimum the editor allows (e.g. 60 s, if under the test budget; otherwise `test.skip` with a reason and cover in owner tests): after the timeout the session reads `parked` without user action.
- [ ] All tests pass and clean up: each test ends its session in `afterEach` (best effort `POST /sessions/{id}/end` for `running`/`parked`).

## Implementation notes
- Files: `frontend/tests/sessions.spec.ts`.
- Passing stub knobs: `setProjectSecret(api, pid, "MARS_STUB_LINE_DELAY_MS", "400")` then `setProfileSecrets(api, project, ["MARS_STUB_LINE_DELAY_MS"])`; the launcher resolves declared secrets and injects them as environment variables. Do not rely on `orchestrator_only` secrets (those are excluded from the container).
- Prefer waiting on visible transcript content (`getByText("Stub reply to: echo me")`) over sleeps; for state, use both the header badge and `waitForSessionState` through the API.
- Cost formatting is the frontend's; assert with a regex tolerant of `$`, trailing zeros and thousands separators.
- The stream reconnect and history-on-reload cases are in the session-view extras task; keep this file to lifecycle and rendering.

## Edge cases
- Container start under Podman on first use can take several seconds; `waitForSessionState(..., "running", 90_000)`.
- If the Session lifecycle epic maps a non-zero CLI exit to `parked` instead of `failed`, assert the documented state (`ARCHITECTURE.md` "Session lifecycle": "unrecoverable CLI or container error → failed") and file a task if the implementation differs.
- Stop while the stub is idle (no delay knob) also parks; only the mid-turn variant proves "stopped" rendering. SIGTERM ("killed") needs a stub that ignores SIGINT, which does not exist; do not test it.
- Optimistic user messages: the `client_id` reconciliation must not leave a duplicate; assert exactly one `Interject` message row.

## Testing
- The spec file; run twice against one stack.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend project and session views": `SessionView`, `Transcript` renderers, `Composer` with `Interject` and answer mode, stop/end/retry controls, header with cost and tokens.
- "Session lifecycle": launcher, owner, `/input`, `/stop`, `/end`, `/retry`; "Claude Code agent backend": translation of the stub's stream-json; "Real-time delivery": session WebSocket.
- "Session container images": stub fixture content and knobs as listed above.

## Correction: `running` on stdin attach, not on `init` (ADR 0032, task 3z8xu)
The pinned CLI writes nothing, `init` included, until its first stdin line (`ARCHITECTURE.md`, "Launch sequence"; `docs/decisions/0032-run-state-on-stdin-attach.md`). Where the text above disagrees, this section wins.
- `creating -> running` happens on stdin attach and queued inputs are flushed there. The header shows no `cli_session_id` until the first turn has produced `init`; the spec asserts that explicitly.
