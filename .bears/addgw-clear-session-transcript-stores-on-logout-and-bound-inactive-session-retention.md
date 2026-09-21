---
id: addgw
title: Clear session transcript stores on logout and bound inactive-session retention
status: done
priority: P2
created: "2026-09-21T10:44:32.387092180Z"
updated: "2026-09-21T22:12:02.866344700Z"
tags:
  - frontend
  - technical-review
  - bug
  - session
  - auth
depends_on:
  - "4srw8"
parent: "579dz"
attempts: 1
---

Problem: frontend/src/session/sessionStore.ts retains per-session stores in a module-level Map. disposeSessionStore is only called by tests. AuthBootstrap clears query and task-board state on sign-out but leaves transcripts, replay cursors and optimistic messages behind. Visiting more sessions grows retained memory, and a later login can reuse a previous login's state.

Acceptance: reset and remove every session store on sign-out; close streams and ensure pending history/input callbacks cannot repopulate state belonging to the old login. Define and implement a bounded inactive-session retention policy while preserving safe resume behavior for retained sessions. Do not evict an actively used store. Cover logout/login isolation, late async completion, and retention/eviction behavior with tests; document the chosen lifecycle.

References: frontend/src/session/sessionStore.ts:885; frontend/src/session/useSessionSocket.ts onSignOut, dispose, start and loadOlder; frontend/src/AuthBootstrap.tsx:77. Contract: SPEC.md, "Frontend", Rules and session state; ARCHITECTURE.md, "Frontend architecture".

Merged from the second review (2026-09-21), same finding, additional detail:
- Concrete leak: user A views session S and logs out; user B logs in in the same tab (navigation is client-side, nothing reloads) and opens S. useSessionSocket.start sees lastSeq > 0, skips REST, and B sees A's folded state including A's pending/rejected optimistic messages. Transcripts are unredacted (ADR 0027). SPEC.md, "Frontend", Rules: sign-out "clears authenticated query and stream stores".
- Also dispose the store when its session is deleted (session/SessionActions.tsx:110).
- Retained-store staleness on a return visit, to settle with the retention policy: pages/SessionPage.tsx:108-111 and :123 let the store's old `session` and `status` win over the fresh REST read (the comment "a store that already holds a session is ahead of this REST read" is false on a return visit), so the header shows the old state until the first `session` frame, or indefinitely if the socket cannot connect. The retained `status: "live"` also lets TerminalView run its attach effect once before start() sets `connecting` (child effects run before the parent's). Set status to `connecting` in dispose() and prefer the loaded session when it is newer.
- The "Not sent" alert from `lastRejection` reappears on returning to a session; clear it with the rest.
- The `reset` store action has no production caller either; the comment at sessionStore.ts:883-884 says useSessionSocket calls it, which contradicts useSessionSocket.ts:117-119 (also listed in the comments task).