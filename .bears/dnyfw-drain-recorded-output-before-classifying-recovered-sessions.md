---
id: dnyfw
title: Drain recorded output before classifying recovered sessions
status: open
priority: P1
created: "2026-09-16T17:54:17.026887Z"
updated: "2026-09-16T17:54:17.026887Z"
depends_on:
  - mpxug
---

Final documentation review mpxug: ARCHITECTURE.md, Restart procedure, reattaches sessions based on DB state without first checking whether the container is still running, and parks a missing container without draining its durable stream.jsonl. A process can finish while the orchestrator is down, leaving unread result/output and stale counters. Specify transcript drain and terminal classification for exited/missing containers before state change; preserve ADR 0010 output recovery independently of ADR 0020 input-delivery limits.