---
id: "6sbzz"
title: "Session transcript: tool calls and subagents start collapsed, with a one-line summary in the header"
status: done
priority: P2
created: "2026-09-20T22:43:49.089530992Z"
updated: "2026-09-21T12:45:00.000000000Z"
tags:
  - frontend
  - session
---

## Summary
Found while walking through the live stack (2026-09-21): every tool row in the session transcript starts expanded and a running subagent starts expanded, so a busy session is mostly tool output. Tool rows and subagent groups start collapsed instead; the collapsed header carries one line saying what the tool was asked (command, file path, pattern), so a folded row is still informative. One click opens the full body, including the result of read/search tools.

## Documents
- `SPEC.md` "Frontend", "Transcript rendering"

## Acceptance criteria
- [ ] `ToolFrame` starts collapsed for every tool; header shows name, summary, status, `truncated` badge; failed rows keep their red border while collapsed.
- [ ] `SubagentGroup` starts collapsed whether the subagent is running or ended; the frame's status mark still shows `running`.
- [ ] Opening a read/search tool row shows its result without a second click.
- [ ] `SPEC.md` updated; frontend quality chain passes.
