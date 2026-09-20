---
id: dyr6h
title: Build TerminalView on xterm.js over binary WebSocket frames with terminal_open/resize/close and exit display
status: in_progress
priority: P2
created: "2026-09-16T20:47:26.061869092Z"
updated: "2026-09-20T08:35:24.576451490Z"
tags:
  - frontend
  - sessions
  - realtime
depends_on:
  - k97mz
parent: cgdc2
attempts: 1
---

## Summary
Add the `Terminal` side panel: an `xterm.js` terminal attached to the session's WebSocket that opens an exec PTY into the running container, forwards keystrokes as binary frames, writes incoming binary frames to the terminal, resizes with the panel through the fit addon, and shows the exit code when the server reports `terminal_closed`. The terminal is an inspection escape hatch only; nothing typed there becomes a transcript event.

## Documents
- `SPEC.md` "WebSocket: session stream": `terminal_open {cols, rows}` (session must be `running`), `terminal_resize {cols, rows}`, terminal data as binary frames both ways, `terminal_close`, `terminal_closed {exit_code}`; the terminal is an `exec` with a PTY running `/bin/bash -l` as the `agent` user multiplexed on the same socket; nothing it does is recorded as events.
- `SPEC.md` "Authentication", stream rules: re-authorization before every incoming application message including terminal bytes; a failed check closes the stream and disposes the terminal attachment.
- `SPEC.md` "User-facing features", Sessions paragraph (open a terminal into a running session's container); "Frontend" stack (`xterm.js` for the terminal view).
- `ARCHITECTURE.md` "Trust boundaries" (the container is the permission boundary) and "Session container specification" (the `agent` user).

## Acceptance criteria
- [ ] `frontend/src/session/TerminalView.tsx` registers as the `Terminal` side panel; `enabled(session)` is `session.state === "running"`; when not running the panel shows `Terminal is available while the session is running`.
- [ ] Opening the panel creates an `@xterm/xterm` `Terminal` (monospace theme matching the console tokens, `convertEol: false`, `cursorBlink: true`) with `@xterm/addon-fit`, mounts it, fits, then sends `terminal_open` with the fitted `cols`/`rows` through the socket API's `terminal.open`.
- [ ] Incoming binary frames (via `terminal.subscribe`) are written with `term.write(bytes)`; `term.onData` encodes with `TextEncoder` and calls `terminal.write`; `term.onBinary` forwards raw bytes.
- [ ] A `ResizeObserver` on the container refits and sends `terminal_resize` (debounced 100 ms) when `cols`/`rows` change.
- [ ] `terminal_closed {exit_code}` prints `\r\n[process exited with code N]` and shows a `Reconnect` button that reopens the PTY; closing the panel, the session leaving `running`, socket reconnection and unmount send `terminal_close` (when the socket is open) and dispose the `Terminal` instance.
- [ ] Reconnection of the socket (status `reconnecting` → `live`) disposes the old attachment and shows `Terminal disconnected — Reconnect` rather than silently reattaching (the server disposes the PTY on close).
- [ ] The xterm CSS is imported once (`@xterm/xterm/css/xterm.css`) and the panel fills the side panel height.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/session/TerminalView.tsx`, registration in `frontend/src/session/sidePanels.ts`.
- Use the socket API from `SessionSocketContext` (previous task): `terminal.open/resize/write/close/subscribe`.
- Keep the `Terminal` instance in a `useRef`, never in React state or Zustand; create it in an effect keyed by `[sessionId, attachGeneration]`.
- Paste: xterm handles bracketed paste; nothing extra.
- No transcript store interaction; the terminal has no history persistence.

## Edge cases
- `terminal_open` sent while the session is not `running` is refused server-side; the hook already guards, but a race (session parks between check and send) results in `terminal_closed` or an `error`; show the closed state.
- Two browser tabs each opening a terminal each get their own PTY; nothing to coordinate client-side.
- Very high output rates: rely on xterm's write buffering; do not call `write` per byte.
- Fonts: fallback to the console monospace stack; ensure `fit()` runs after fonts load (`document.fonts.ready`).

## Testing
- No Vitest for xterm (needs a real DOM canvas); a unit test for the resize debounce helper if extracted. The terminal is exercised end-to-end by the End-to-end tests epic against the stub image, and the backend PTY by the Real-time delivery epic's engine tests.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Real-time delivery epic: terminal multiplexing on the session socket as documented.
- Repository scaffolding epic: `@xterm/xterm` and `@xterm/addon-fit` installed.