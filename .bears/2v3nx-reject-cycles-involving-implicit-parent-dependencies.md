---
id: "2v3nx"
title: Reject cycles involving implicit parent dependencies
status: open
priority: P2
created: "2026-09-16T17:54:17.226659Z"
updated: "2026-09-16T17:54:17.226659Z"
depends_on:
  - mpxug
---

Final documentation review mpxug: docs/data-model.md, tasks and task_dependencies, blocks parents on open children but checks cycles only among explicit blocks edges. A child depending on its parent passes that check while both remain blocked. Specify cycle validation over explicit prerequisites plus parent-to-child waiting edges for dependency edits, task creation and re-parenting. Keep discovered_from and related out of readiness; add acceptance for child-to-parent and longer mixed cycles.