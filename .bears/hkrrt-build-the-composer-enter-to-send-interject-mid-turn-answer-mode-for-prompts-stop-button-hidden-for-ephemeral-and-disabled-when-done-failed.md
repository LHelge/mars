---
id: hkrrt
title: "Build the Composer: Enter to send, Interject mid-turn, answer mode for prompts, stop button, hidden for ephemeral and disabled when done/failed"
status: in_progress
priority: P1
created: "2026-09-16T20:46:16.473419671Z"
updated: "2026-09-20T07:58:59.424962848Z"
tags:
  - frontend
  - sessions
depends_on:
  - zum7c
parent: cgdc2
attempts: 1
---

## Summary
Build the session `Composer`: a text area that sends a `message` input on Enter (Shift+Enter inserts a newline), relabels its button `Interject` while a turn is in progress, switches to answer mode when a `prompt` event is pending (sending an `answer` with `reply_to`), offers a stop button, is absent for ephemeral sessions and disabled with an explanation when the session is `done` or `failed`. It uses the socket hook's `send`/`stop` and the store's `turnActive`/`pendingPrompt`.

## Documents
- `SPEC.md` "Frontend", "Composer": text area with submit on Enter (Shift+Enter for newline), disabled when the session is `done`/`failed` and absent for ephemeral sessions; while a turn is in progress the button reads "Interject"; a stop button sends `stop`; when a `prompt` event is pending the composer switches to answer mode and sends an `answer` with `reply_to`.
- `SPEC.md` "WebSocket: session stream": `SessionInput` (`message` | `answer {reply_to, text}`); a `message` is accepted for a conversational session in `creating`, `running` or `parked`; ephemeral sessions reject all additional input; an `answer` whose prompt has been consumed is rejected (`input_rejected`).
- `SPEC.md` "AgentEvent": `prompt {prompt_id, text, options?}`.
- `SPEC.md` "User-facing features", Sessions paragraph (a parked session looks like a running one that is waiting; sending a message relaunches it); "Sessions" table (`POST /sessions/{id}/input` 202 relaunches if parked; 409 for ephemeral).
- `ARCHITECTURE.md` "Session lifecycle" table (which states accept input) and "Stop semantics" (SIGINT then SIGTERM after grace; session becomes `parked`); ADR 0020 (no delivery guarantee across restarts; user may resend).

## Acceptance criteria
- [ ] `frontend/src/session/Composer.tsx` renders nothing when `session.kind === "ephemeral"`.
- [ ] When `session.state` is `done` or `failed` the text area and button are disabled with the note `Session has ended` / `Session failed — use Retry to relaunch`.
- [ ] Enter submits (trimmed non-empty text), Shift+Enter inserts a newline; the text area auto-grows to 8 lines then scrolls; Cmd/Ctrl+Enter also submits.
- [ ] Button label is `Send` normally, `Interject` while `turnActive` is true and the session is `running`; the hint under the box reads `Sending will relaunch the session` when `parked`, `Queued until the session starts` when `creating`.
- [ ] Answer mode when `pendingPrompt` is set: a banner shows the prompt text; if `options` exist they render as buttons that send `{kind: "answer", reply_to: pendingPrompt.seq, text: option}`; the text area sends a free-text answer with the same `reply_to`; a `Dismiss` control calls `consumePrompt()` and returns to message mode without sending.
- [ ] A `Stop` button (visible only when `running`) calls `stop()` and shows `Stopping…` until a `state_change`/`session` message moves the session to `parked` (button re-enables after 30 s regardless).
- [ ] After a successful `send` the text area clears and refocuses; the optimistic message appears in the transcript immediately (store behaviour); an `input_rejected` reason surfaces as an `Alert` above the composer and the text is restored so the user can edit and resend.
- [ ] `onResend` from the transcript (rejected message) pre-fills the composer with that text.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/session/Composer.tsx`, `frontend/src/session/Composer.test.tsx`, `frontend/src/session/index.ts`.
- Props: `Composer({ sessionId, socket: ReturnType<typeof useSessionSocket>, initialText?, onConsumedInitialText })`; reads `session`, `turnActive`, `pendingPrompt` through store selectors.
- The store's `inputRejected` action already marks the optimistic message; the composer additionally subscribes to the last rejection (add `lastRejection: {client_id, reason} | null` to the store if not present, cleared on the next send) to restore text.
- Keyboard handling on `keydown`: `Enter && !shiftKey && !isComposing` → submit; IME composition must not submit.
- No REST calls here beyond the socket hook's fallbacks.

## Edge cases
- Multiple `prompt` events before an answer: the store keeps only the latest as `pendingPrompt`; the banner shows that one.
- A `prompt` answered by someone else (another browser): the `user_message` with `reply_to` clears `pendingPrompt` in the store and the composer drops answer mode.
- Pasting very large text (over 100 KiB): warn but allow.
- While `reconnecting`, sending still works through the REST fallback; show a subtle `offline, sending over HTTP` hint.

## Testing
- Vitest + `@testing-library/react`: render with a mocked socket API and a store in each state; assert hidden for ephemeral, disabled for done/failed, `Interject` label with `turnActive`, Enter vs Shift+Enter, answer mode payload `{kind:"answer", reply_to}`, option buttons, stop button visibility.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `Alert`, `SubmitButton`.

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- The composer has no answer mode and the store has no `pendingPrompt`: it only ever sends `{kind:"message"}`. Rescope the task to the plain composer; a question the model writes is ordinary assistant text.
