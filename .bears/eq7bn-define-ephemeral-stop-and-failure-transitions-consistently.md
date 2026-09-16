---
id: eq7bn
title: Define ephemeral stop and failure transitions consistently
status: open
priority: P2
created: "2026-09-16T17:54:17.111997Z"
updated: "2026-09-16T17:54:17.111997Z"
depends_on:
  - mpxug
---

Final documentation review mpxug: ARCHITECTURE.md, Stop semantics and Claude Code invocation, parks every stopped or authentication-failed session; the process model and docs/data-model.md prohibit ephemeral sessions from ever parking or resuming. Specify terminal outcomes for ephemeral stop, clean exit without result, authentication failure and missing container, and align SPEC.md session endpoints and lifecycle diagram.