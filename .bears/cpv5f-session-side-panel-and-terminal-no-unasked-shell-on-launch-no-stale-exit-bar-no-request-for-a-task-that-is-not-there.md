---
id: cpv5f
title: "Session side panel and terminal: no unasked shell on launch, no stale exit bar, no request for a task that is not there"
status: done
priority: P2
created: "2026-09-21T10:53:21.634646954Z"
updated: "2026-09-21T19:25:53.769045859Z"
tags:
  - frontend
  - technical-review
  - bug
  - session
parent: "579dz"
attempts: 1
---

Problem:
- session/SidePanel.tsx:38 `useState(entries[0]?.id)` is evaluated once. Both launch flows navigate to /sessions/:id while the session is `creating`, where Changes is disabled (sidePanels.ts:48), so entries[0] is `terminal`. On a screen >= 1024 px the panel starts open, and once the session runs and the socket is live TerminalView loads the xterm chunk and sends terminal_open: every launched session gets a `/bin/bash -l` exec nobody asked for, and the lazy load is defeated.
- session/TerminalView.tsx:62 and :78-161: exitCode is reset only by reconnect(). The shell exits and the bar shows; the session parks and later runs again, the effect re-attaches a new terminal (running is a dependency) and the bar still says the process exited.
- session/TasksPanel.tsx:43-51: refetch() ignores `enabled` (:38). For a session with task_id null, queryFn calls getTask(pid, "") — a 404 on every `session` frame while the Tasks tab is open.
- TasksPanel.tsx:53-54, :83-104 probably lists the launched task twice: a launch claims the task, a claim writes the session link, so GET /sessions/{id}/tasks includes it under "Touched" as well as "Launched for", and the "Nothing beyond the task above." branch is unreachable (server side unverified; the panel has no test). It also fetches a full TaskDetail with comments and hand-offs for a title, state and lease, keyed by UUID while the drawer keys the same task by number, so drawer mutations never invalidate it.
- session/SidePanel.tsx:67-102 has role="tab"/"tabpanel" without aria-controls, aria-labelledby or arrow-key/roving-tabindex handling.
- There are two Stop buttons with different behaviour: Composer.tsx:196-213 has a `stopping` state and a 30 s timeout, SessionActions.tsx:126-138 has no pending state, so pressing the header button gives no feedback.

Acceptance: the active tab is `string | null` set only by a click, with the shown tab derived (`entries.find(...) ?? first enabled`), and the terminal never attaches until the user has selected it; a launched session opens on Changes once it is enabled. exitCode clears when a terminal attaches. TasksPanel refreshes through invalidateQueries (which skips disabled queries), filters the launched task out of the touched list and takes its row from that list where possible; add a component test. Either complete the ARIA tab pattern or use buttons with aria-pressed. One Stop behaviour shared by both buttons. Invoke the frontend-design skill if the panel's default changes what users see first.

References: frontend/src/session/SidePanel.tsx, sidePanels.ts, TerminalView.tsx, TasksPanel.tsx, Composer.tsx, SessionActions.tsx; frontend/src/pages/project/LaunchSessionForm.tsx and tasks/LaunchForTask.tsx (the navigations). Contract: SPEC.md, "WebSocket: session stream" (terminal_open) and "Frontend", session page.