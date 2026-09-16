---
id: gzgkn
title: Specify durable auditing for project-only git operations
status: open
priority: P2
created: "2026-09-16T17:54:17.169034Z"
updated: "2026-09-16T17:54:17.169034Z"
depends_on:
  - mpxug
---

Final documentation review mpxug: README.md promises attributed audit of pushes/merges and ADR 0007 requires an audit row. SPEC.md permits REST merge/rebase/push using integration branches without a session or task. docs/data-model.md has only session-scoped events and tracker TaskEvents, with no project git audit storage; AgentEvent.git detail also has no actor contract. Define the durable record and actor schema for these operations or explicitly revise the audit promise through an ADR.