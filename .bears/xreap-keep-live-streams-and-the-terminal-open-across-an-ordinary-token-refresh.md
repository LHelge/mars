---
id: xreap
title: Keep live streams and the terminal open across an ordinary token refresh
status: open
priority: P1
created: "2026-09-21T10:51:19.647851173Z"
updated: "2026-09-21T10:51:19.647851173Z"
tags:
  - frontend
  - technical-review
  - bug
  - auth
  - session
depends_on:
  - "4srw8"
parent: "579dz"
---

Problem: services/auth.ts installSession fires the onCredentialsReplaced handlers whenever a token was already present, which is every ordinary 401-then-refresh rotation and not only a self-service password change. SessionSocket.onCredentialsReplaced (session/useSessionSocket.ts:307-314, whose own comment says "A self-service password change installed a new pair") is guarded only by the socket's own `refreshing` flag and unconditionally runs teardown(1000) and connect(); TaskStream does the same (tasks/useTaskStream.ts:97). The access token lives 15 minutes. After that any REST 401 — the window-focus refetch of the session detail, the TasksPanel poll, a Sync click — refreshes through apiClient and closes a healthy socket. The exec PTY multiplexed on that socket is disposed server-side: the user's shell, cwd and any foreground build are lost and TerminalView shows "Terminal disconnected". The board meanwhile flips to "reconnecting" and re-reads its snapshot. It repeats on every rotation. The teardown is unnecessary: SPEC.md, "Authentication" says an open stream is not closed because its token expired.

Acceptance: a refresh rotation does not close an open session socket or task stream; a password change still does (or relies on the server's 1008 close at the next ping plus the existing close path — choose one and document it). Give installSession a reason (login / refresh / password_change) or an equivalent split so the handlers fire only when the credentials were really replaced. A stream that connects later still reads the current token at connect time. Tests: with an open socket and terminal, a REST 401 -> refresh leaves the socket and PTY untouched and the store status stays `live`; a password change reconnects exactly once.

References: frontend/src/services/auth.ts:127-151; frontend/src/session/useSessionSocket.ts:123 and :307; frontend/src/tasks/useTaskStream.ts:97; frontend/src/components/PasswordChangeForm.tsx (the intended caller); frontend/src/session/TerminalView.tsx. Contract: SPEC.md, "Authentication" and "WebSocket: session stream"; ARCHITECTURE.md, "Frontend architecture". Depends on 4srw8, which reworks the same function.