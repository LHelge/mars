---
id: bxhas
title: Build sessionStore (Zustand) folding AgentEvents into messages, order, pendingTools, subagents and pendingPrompt, with fixture tests
status: in_progress
priority: P0
created: "2026-09-16T20:41:39.804562669Z"
updated: "2026-09-20T07:37:32.868687667Z"
tags:
  - frontend
  - sessions
  - realtime
  - tests
depends_on:
  - "2txez"
parent: cgdc2
attempts: 1
---

## Summary
Implement the per-session Zustand store that folds every `AgentEvent` kind into display messages as it arrives and never retains a raw event array (ADR 0022's transcript rule; `SPEC.md` "Session state"). The store is pure state plus reducer-style actions so it can be unit-tested with hand-written fixture event sequences independently of the WebSocket.

## Documents
- `SPEC.md` "Frontend", "Session state": the `SessionState` interface (`session, status, lastSeq, order, messages, pendingTools, subagents, pendingPrompt`), the `Message` union, and the folding rules (`text_delta` appends to the current streaming assistant message; the following `text` replaces it and clears `streaming`; `tool_call` creates a tool message and registers it in `pendingTools`; `tool_result` completes it; events carrying `parent_tool_use_id` are placed under the tool message with that id instead of at the top level; `user_message` with a `client_id` matching an optimistic message replaces it).
- `SPEC.md` "AgentEvent" (every kind and its fields; `parent_tool_use_id` copied onto everything emitted inside a subagent; `result` carries `cost_usd`, `usage`, `num_turns`, `duration_ms`, `is_error`, `subtype`; `state_change` carries `from`, `to`, `reason`, `signal?`).
- `SPEC.md` "WebSocket: session stream" (`input_accepted {client_id, seq}`, `input_rejected {client_id, reason}`; the client keeps the highest `seq` and dedupes on it); "Sessions" (`GET /sessions/{id}/events?before=&limit=` newest-last, `has_more`).
- `ARCHITECTURE.md` "Frontend architecture" (folded into `messages` map plus order, `pendingTools`, `status`; raw arrays not retained); "Event delivery" (older history is fetched over paginated REST with `seq` as the cursor; dedupe on `seq`).
- ADR 0022 (session transcript streaming unchanged: fold, do not buffer).

## Acceptance criteria
- [ ] `frontend/src/session/sessionStore.ts` exports `createSessionStore()` (factory, so tests get isolated instances) and a `useSessionStore` hook bound to the current session id, plus the `Message` union: `user {id, kind:"user", text, pending?, rejected?: string, reply_to?}`, `assistant_text {text, streaming}`, `thinking {text, redacted}`, `tool {name, input, result?, is_error?, truncated?, running, children?: string[], subagent?: {description, agent_type?, is_error?}}`, `system {text, level: "info" | "warn" | "error", detail?: unknown}`, `result {subtype, is_error, num_turns, duration_ms, cost_usd?, usage?}`, `raw {native}`.
- [ ] State holds exactly the SPEC `SessionState` fields plus these documented internal cursors: `oldestSeq: number | null`, `hasMore: boolean`, `gitEventSeq: number` (seq of the last `git` event, consumed by the Changes panel), `turnActive: boolean`. No field ever holds an `AgentEvent[]`.
- [ ] Actions: `reset()`, `setSession(session)`, `setStatus(status)`, `applyEvent(event)` (live/replay path, ignores `seq <= lastSeq`, advances `lastSeq`), `prependHistory(events, has_more)` (older page, newest-last order as returned; sets `oldestSeq`), `addOptimisticUser(client_id, input)` (message id `client:<client_id>`), `inputAccepted(client_id, seq)`, `inputRejected(client_id, reason)`, `consumePrompt()`.
- [ ] Folding rules, each covered by a test: `init` → system info `Session started` / `Session resumed (…)` naming the model when present; `user_message` → user message (replaces `client:<client_id>` when it matches, keeping the optimistic message's position in `order`); `text_delta` → append to the newest `assistant_text` with `streaming: true` in the same scope (top level or the same `parent_tool_use_id`), creating one if none; `text` → replace that streaming message's text and set `streaming: false`, or create a new one; `thinking` → thinking message; `tool_call` → tool message `running: true`, `pendingTools[tool_use_id] = message id`; `tool_result` → completes the pending tool (`result`, `is_error`, `truncated`, `running: false`) and removes it from `pendingTools`; a result with no pending tool creates a tool message named `unknown` so nothing is dropped; `subagent_start` → marks the tool message for that `tool_use_id` as a subagent with `children: []` and `subagents[tool_use_id] = []`; `subagent_end` → sets `subagent.is_error`; any event with `parent_tool_use_id` is appended to `subagents[parent]` and the parent's `children`, never to `order` (a parent never seen creates a placeholder subagent tool message named `Agent`); `permission_denied` → system warn `Permission denied: <name> — <reason>`; `prompt` → system info with the prompt text and options, and `pendingPrompt = {seq, prompt_id}`; `result` → result message, sets `turnActive = false`, clears any streaming flag, clears `pendingPrompt`; `error` → system error (`fatal` noted); `state_change` → system info `Stopped (SIGINT)` / `Killed (SIGTERM)` / `<from> → <to>: <reason>` and updates `session.state` when a session is loaded; `launch_warning` → system warn; `git` → system info (ok) or warn (not ok) with `op` and `detail`, and `gitEventSeq = seq`; `raw` → raw message.
- [ ] `turnActive` becomes true on `user_message`, `text_delta`, `text`, `thinking`, `tool_call`, and false on `result`, `error` with `fatal`, and `state_change` to `parked`/`done`/`failed`.
- [ ] `applyEvent` with a duplicate or lower `seq` than `lastSeq` is a no-op (reconnect replay dedupe); `prependHistory` accepts only events with `seq < oldestSeq` (or any when `oldestSeq` is null) and merges a `tool_call` from the older page into an existing `unknown` tool message with the same `tool_use_id` if one was created earlier.
- [ ] Message ids for event-derived messages are `e<seq>`; a `text` that replaces a streaming message keeps the streaming message's id so `order` is stable during streaming.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/session/sessionStore.ts`, `frontend/src/session/sessionStore.test.ts`, `frontend/src/session/fixtures/*.json` (hand-written `AgentEvent[]` sequences: `simple_turn.json` (init, user_message, text_delta×3, text, result), `tool_use.json` (tool_call Edit, tool_result, tool_call Bash, tool_result is_error), `subagent.json` (tool_call Task + subagent_start, nested text/tool_call/tool_result with `parent_tool_use_id`, subagent_end, tool_result), `prompt_and_answer.json`, `stop_and_park.json` (state_change with signal), `git_ops.json`, `raw_and_unknown.json`), `frontend/src/session/index.ts` barrel.
- Use `zustand`'s `create`/`createStore` with a vanilla store for tests (`createSessionStore()` returns a `StoreApi<SessionStore>`; `useSessionStore` wraps it with `useStore`). Keep one store instance per mounted session id in a module-level `Map<string, StoreApi>` so navigating between sessions does not leak folded state; `reset()` is called by the socket hook when the id changes.
- Write the fold as a pure function `foldEvent(state: SessionState, event: AgentEvent): SessionState` used by both `applyEvent` and `prependHistory` (the latter folds the older page into an empty scratch state, then concatenates: `order = older.order.concat(state.order)`, merges `messages`, `subagents`, and reconciles `pendingTools`/`unknown` tool messages). Immutable updates: copy the touched record entries only, never the whole `messages` map per event beyond a shallow copy.
- Scope resolution for `text_delta`/`text`: the "current streaming message" is searched from the end of the scope's list (`order` or `subagents[parent]`), stopping at the first non-`assistant_text` message; this keeps a delta from attaching to an assistant message from an earlier turn.
- `Message.result` for tool messages keeps `content` as delivered (`string | unknown`); renderers decide how to display it.
- Fixture JSON files are frontend-owned test data shaped per `SPEC.md`; they are not the backend translation fixtures under `orchestrator/tests/fixtures/claude/`.

## Edge cases
- Events may arrive with `message_id` shared across blocks of one assistant message; do not merge on it (blocks are separate messages).
- `tool_result.content` may be a string or structured JSON; store as-is.
- A `text_delta` after `text` for the same message (out-of-order backends) starts a new streaming message rather than corrupting the completed one.
- `subagent_start` may arrive before or after the matching `tool_call` in the same batch; both orders must produce one tool message.
- `user_message` with `reply_to` clears `pendingPrompt` when `reply_to === pendingPrompt.seq`.
- `inputRejected` marks the optimistic message `pending: false, rejected: reason` and keeps it visible so the user can resend (ADR 0020: no delivery-status UI beyond this).
- `reset()` must also clear `oldestSeq`, `hasMore`, `gitEventSeq`, `turnActive`.

## Testing
- Vitest unit tests, one `describe` per fixture: load the fixture, `applyEvent` each, assert `order`, `messages` (snapshot-free explicit assertions on ids, kinds, text, `running`, `streaming`), `pendingTools` empty at the end, `subagents` nesting, `pendingPrompt`, `turnActive`, `lastSeq`.
- Dedupe test: applying the same fixture twice yields the same state. Reconnect test: apply events 1–5, then 3–8, assert `order` has eight entries.
- Prepend test: fold events 6–10 live, then `prependHistory(events 1–5)`; assert order is 1–10 and a `tool_result` at seq 6 whose `tool_call` is at seq 5 ends as one completed tool message.
- Optimistic test: `addOptimisticUser`, then `user_message` with the same `client_id` → one user message, `pending` false; `inputRejected` path.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written (Vitest is delivered by the frontend scaffold task).

## Assumes from other epics
- Frontend foundation epic: nothing at runtime; the store imports only `types/`.
- Repository scaffolding epic: `frontend/` Vite project with the Vitest runner (`npm run test:unit`).

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- The session store has no `pendingPrompt`; drop it from the state shape, the fold rules and the tests.
