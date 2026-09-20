---
id: zum7c
title: "Implement useSessionSocket: WebSocket with after=lastSeq, token-refresh reconnect, seq dedupe, REST history paging and input/stop/terminal sending"
status: in_progress
priority: P1
created: "2026-09-16T20:43:20.254778222Z"
updated: "2026-09-20T07:49:41.269793552Z"
tags:
  - frontend
  - sessions
  - realtime
  - tests
depends_on:
  - bxhas
parent: cgdc2
attempts: 1
---

## Summary
Implement the hook that owns a session's WebSocket: initial history through REST, the socket opened with `after = store.lastSeq`, dispatch of every server message into `sessionStore`, reconnection with a refreshed access token (never the browser's stale URL), older-history paging on scroll-up, and typed senders for `input`, `stop` and the terminal messages. It is the single place in the frontend that talks to `/ws/sessions/{id}`; `SessionView`, `Composer` and `TerminalView` consume it.

## Documents
- `SPEC.md` "WebSocket: session stream": `GET /ws/sessions/{id}?after=<seq>&token=<jwt>`; replay `seq > after` then live; client keeps the highest `seq` and dedupes on it; JSON text frames except terminal data as binary frames; server messages `event`, `session`, `input_accepted`, `input_rejected`, `terminal`, `terminal_closed`, `error` (followed by close); client messages `input {client_id, input}`, `stop`, `terminal_open {cols, rows}` (session must be `running`), `terminal_resize`, terminal binary data, `terminal_close`; `client_id` is client-generated and echoed back; server pings every 30 s.
- `SPEC.md` "Authentication", stream rules: `?token=`; the frontend disables automatic retry, refreshes the access token and reopens with a fresh token and the last cursor; a 401 from refresh clears local authentication, closes other streams and routes to login; transient failures keep normal backoff; a successful self-service password change installs the new pair before reopening streams; `error` with `authentication required` then close 1008 on revocation.
- `SPEC.md` "Frontend", "Session state": `useSessionSocket(sessionId)` opens with `after = store.lastSeq`, reconnects itself with a fresh access token on close, fetches older history through REST when the user scrolls up, dispatches every event to the store; `status: "connecting" | "live" | "reconnecting"`.
- `SPEC.md` "Sessions": `GET /sessions/{id}/events?before=<seq>&limit=<n≤500>` → `{events, has_more}` newest-last, ending just before `before`; `POST /sessions/{id}/input` 202 as the REST equivalent.
- `SPEC.md` known v1 restart limitation (ADR 0020): `input_accepted` is acceptance, not delivery; no automatic resend after reconnect.
- `ARCHITECTURE.md` "Event delivery" (subscribe-before-replay on the server; client dedupes on `seq`; older history over paginated REST with `seq` cursor).

## Acceptance criteria
- [ ] `frontend/src/session/useSessionSocket.ts` exports `useSessionSocket(sessionId: string): { status, send(input: SessionInput): string /* client_id */, stop(), loadOlder(): Promise<void>, terminal: { open(cols, rows), resize(cols, rows), write(bytes: Uint8Array), close(), subscribe(listener: (frame: Uint8Array | {exit_code: number}) => void): () => void } }`.
- [ ] On mount (and when `sessionId` changes): `reset()` the store, load the newest page with `listEvents(id, {limit: 200})`, fold it with `prependHistory`, set `lastSeq` to the highest seq seen (0 when empty), then open the socket with `after=lastSeq`. Status is `connecting` until the socket's `open` event, then `live`.
- [ ] Every `event` message goes to `applyEvent` (the store dedupes on `seq`); `session` → `setSession`; `input_accepted` → `inputAccepted`; `input_rejected` → `inputRejected`; `terminal_closed` and binary frames → terminal subscribers; `error` → if `message === "authentication required"` treat as auth failure (below), otherwise show it as a system error message and let the close handler reconnect.
- [ ] On `close` not initiated by the hook: status `reconnecting`; call the foundation's `refreshAccessToken()`; on success reopen with `after = store.lastSeq` and the new token; on a 401 from refresh do nothing further (the foundation clears auth and routes to login); on network failure back off exponentially 1 s, 2 s, 4 s … capped at 30 s with ±20 % jitter, retrying indefinitely while mounted. Never rely on the browser reconnecting with the old URL.
- [ ] Subscribes to the foundation's auth-change notification so that a token pair replaced by a self-service password change closes and reopens the socket with the new token; a cleared token closes it without reconnecting.
- [ ] `send(input)` generates `client_id` with `crypto.randomUUID()`, calls `addOptimisticUser(client_id, input)` first, then sends `{type:"input", client_id, input}`; if the socket is not open it falls back to `sendInput(id, input)` over REST (202) and the optimistic message is reconciled by the replayed `user_message`. Returns the `client_id`.
- [ ] `stop()` sends `{type:"stop"}` when open, else `stopSession(id)` over REST.
- [ ] `loadOlder()` is a no-op when `hasMore` is false or a page is in flight; otherwise `listEvents(id, {before: oldestSeq, limit: 200})` → `prependHistory(events, has_more)`.
- [ ] Terminal: `open` sends `terminal_open` only when `store.session.state === "running"`; `write` sends a binary frame (`ArrayBuffer`); incoming binary frames (`binaryType = "arraybuffer"`) are delivered to subscribers as `Uint8Array`; `close` sends `terminal_close`; unmount closes the terminal first, then the socket.
- [ ] Unmount closes the socket with code 1000 and cancels pending backoff timers; the store for that session id is kept in memory so returning to the page resumes from `lastSeq` (the hook reopens with that cursor and REST is skipped when `lastSeq > 0`).
- [ ] Unit tests with a fake `WebSocket` (class injected through an optional `factory` parameter or a module-level `setWebSocketFactory` used only in tests) cover: initial REST load then `after` cursor; dedupe of an event replayed after reconnect; reconnect calls refresh then reopens with the new token and current `lastSeq`; 401 on refresh stops retrying; `send` adds an optimistic message before the frame is sent; `terminal_open` refused when not running.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/session/useSessionSocket.ts`, `frontend/src/session/socketUrl.ts` (`buildSessionSocketUrl(id, after, token)` → `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws/sessions/${id}?after=${after}&token=${encodeURIComponent(token)}`), `frontend/src/session/useSessionSocket.test.ts`, `frontend/src/session/index.ts`.
- Keep the connection logic in a plain class `SessionSocket` (no React) with `connect()`, `close()`, callbacks, so the tests drive it without rendering; `useSessionSocket` wraps it in `useEffect`/`useRef` and exposes stable callbacks with `useCallback`.
- Message parsing: `JSON.parse` in a try/catch; a malformed frame is logged with `console.warn` and ignored.
- `send` and `stop` must not throw when the socket is closed; they use the REST fallback so the UI behaves during a reconnect.
- The token must never appear in `console` output or in React state that is rendered.
- Do not store `WebSocket` instances in Zustand.

## Edge cases
- The server closes with code 1008 after `authentication required`: treat as auth failure (refresh then reconnect once; if the socket is closed again with 1008 within 5 s, stop and let the foundation route to login).
- `session` messages can arrive before the first `event`; `setSession` must be safe with an empty transcript.
- A `session` message with `state: running` after a `parked` one means a relaunch; nothing to reset.
- `loadOlder` when `oldestSeq` is 1 sets `hasMore` false without a request.
- Two mounted consumers of the same session (transcript and a second tab) share the store; the hook must be mounted once per page (`SessionView` owns it and passes the API down via context).
- `after` must be the store's `lastSeq` at the moment of (re)connection, not a captured value.

## Testing
- Vitest with the fake socket described above; assert URLs, frames sent, store state after each step, and timer behaviour with `vi.useFakeTimers()`.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `services/auth` exposing `getAccessToken()`, `refreshAccessToken(): Promise<string>` (rejects with a 401-carrying error when the refresh fails and already performs the clear-and-route-to-login side effects), and `subscribeAuth(listener)` returning an unsubscribe function.
- Real-time delivery epic: the WebSocket endpoint and message contracts as documented; the scaffold's Vite proxy forwards `/ws` with `ws: true`.