---
id: h2uej
title: Memoise transcript rows so a streamed delta re-renders one row, not the viewport
status: done
priority: P2
created: "2026-09-21T10:53:09.024281911Z"
updated: "2026-09-21T20:46:05.885776133Z"
tags:
  - frontend
  - technical-review
  - performance
  - session
  - react
parent: "579dz"
attempts: 1
---

Problem: the transcript's comments assume a React Compiler that is not installed (vite.config.ts uses plain react(); package.json has no compiler dependency), and the only memo() in the application is TaskCard. Transcript subscribes to tailLength, so every text_delta re-renders it and builds fresh <MessageRow> elements for the viewport plus 16 overscan rows (session/Transcript.tsx:38-55). Each of those rows re-parses its markdown because MarkdownBody is not memoised (components/Markdown.tsx:46), and each open Edit row recomputes the O(n*m) LCS lineDiff (session/tools/EditToolRenderer.tsx:35, which also runs the same four guards twice per render, :31-82 and :93-97). The store side is already right for this — every row subscribes to messages[id] / subagents[toolUseId] only and every selector returns a stable reference — so the work is wasted purely for want of memo. Plausible second cost: the inline getItemKey (Transcript.tsx:54) changes identity every render, which resets the virtualizer's measurement memo so each render rebuilds measurements for all rows.
Fold cost, fine at hundreds of messages and noticeable at tens of thousands or on the after=0 full-replay fallback: every tool_call scans all messages because a miss in pendingTools is the normal case (sessionStore.ts:203-215), every result scans for streaming rows (:305-314), and every delta spreads the whole messages record.

Acceptance: MessageRow is memo()-wrapped (its props are already stable; setResend has a stable identity) and MarkdownBody does not re-parse unchanged text; lineDiff is memoised on its inputs; getItemKey is stable per `order`. Either adopt the React Compiler deliberately (ARCHITECTURE.md, "Frontend architecture", plus the lint rule that comes with it) or make the comments describe the memoisation that exists — not both half-way. A render-count test (or React Profiler assertion in a component test) proves a delta to row N re-renders row N only. Optionally add a toolUseId -> messageId index that survives completion; measure before and after with a long fixture and record the numbers in the commit.

References: frontend/src/session/Transcript.tsx, messages/MessageRow.tsx, tools/EditToolRenderer.tsx, sessionStore.ts; frontend/src/components/Markdown.tsx; frontend/vite.config.ts. Contract: SPEC.md, "Frontend", transcript rendering; ARCHITECTURE.md, "Frontend architecture".