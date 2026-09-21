---
id: vdscb
title: Frontend design pass (placeholder)
type: epic
status: cancelled
priority: P2
created: "2026-09-17T07:01:33.989747596Z"
updated: "2026-09-21T11:29:38.174188258Z"
tags:
  - frontend
  - design
  - placeholder
depends_on:
  - "2f5u2"
  - cgdc2
  - gn4y2
---

Placeholder for a frontend design pass once the foundation, project/session views, and task board/detail/hand-off controls are implemented and can be reviewed together in the browser.

Decided by the user on 2026-09-20: the design review comes last, after the End-to-end tests with Playwright epic (6s8j7), so the user can first test the app manually. When it becomes ready, pause for a design review with the user and decide the scope then. Do not plan, break down, or implement this epic in advance beyond the small findings already filed under it.

Because the E2E specs exist by then, the design pass keeps them green: a change to a label, role or structure a spec selects on updates that spec in the same commit.

Reference: `SPEC.md`, "Frontend"; `CLAUDE.md`, "Frontend conventions". No design decisions are defined yet.

**Closed by the user on 2026-09-21 without a design pass:** the frontend looks good enough as it is for now. The one finding filed under it, `dsfs3` (StatusBadge colour for `error`, PasswordChangeForm autofocus on `/settings`), was detached and stays open as a standalone task. A later design pass is a new epic.