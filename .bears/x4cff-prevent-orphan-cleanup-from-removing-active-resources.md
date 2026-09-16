---
id: x4cff
title: Prevent orphan cleanup from removing active resources
status: open
priority: P1
created: "2026-09-16T17:54:16.965459Z"
updated: "2026-09-16T17:54:16.965459Z"
depends_on:
  - mpxug
---

Final documentation review mpxug: ARCHITECTURE.md, Background jobs, deletes /data/tmp leftovers without excluding active merge/rebase clones or startup probes. It also treats parked-session containers as orphaned, although resume creates a container while the row stays parked until init. Specify ownership/locking and revalidation shared by cleanup and launch; acceptance must cover a cleanup tick during merge and during resume.