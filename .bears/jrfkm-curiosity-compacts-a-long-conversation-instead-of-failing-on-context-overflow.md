---
id: jrfkm
title: Curiosity compacts a long conversation instead of failing on context overflow
status: open
priority: P2
created: "2026-09-21T12:23:12.342885Z"
updated: "2026-09-21T12:23:12.342885Z"
tags:
  - curiosity
  - backlog
  - agent-backends
depends_on:
  - dtq74
---

## Summary
midgaard had no compaction by design: sessions were task-scoped and handed off on context overflow (`ModelError::is_context_overflow`). A Mars conversational session lives for hours or days (ADR 0003), so Curiosity needs a strategy. Design first, then build.

## Acceptance criteria
- [ ] A short design note in `curiosity/docs/` choosing between: summarise-and-truncate at a threshold of the model's window, dropping old tool results first, or both; how the window size is known per `provider/model` (OpenRouter's models endpoint reports it); what is persisted so `session/load` reproduces the compacted history rather than re-expanding it.
- [ ] Compaction happens between turns or between model calls, never mid-stream; it is visible to the client as an event (ACP update or `_meta`) that the Mars adapter maps to a `raw` or a dedicated additive event — decide with `SPEC.md`, "AgentEvent".
- [ ] A provider's context-overflow error triggers one compaction and one retry before the turn fails.
- [ ] The compaction call's tokens and cost are part of the turn's usage.

## Testing
- Mock-provider tests with a tiny configured window; a load-after-compaction test.