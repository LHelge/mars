---
id: "2v47s"
title: Render thinking text and subagent reports as markdown; keep user messages literal
status: done
priority: P2
created: "2026-09-20T23:43:14.045455211Z"
updated: "2026-09-21T14:25:16.358283993Z"
tags:
  - frontend
  - session
  - markdown
depends_on:
  - tvcf5
parent: qprj6
attempts: 1
---

## Summary
Some agent-written text in the transcript bypasses `MarkdownBody` and shows raw markdown: thinking text (`ThinkingMessage`, a pre-wrapped italic paragraph) and the final report of a subagent (the `Task`/`Agent` tool's result, drawn by `ToolResult` as monospace text). Route them through `MarkdownBody`. Decide explicitly, and document, what stays literal.

## Documents
- `SPEC.md` "Frontend": "Transcript rendering" and the "Markdown" paragraph — the list of where markdown is and is not rendered

## Acceptance criteria
- [ ] `ThinkingMessage` renders its text with `MarkdownBody` inside the existing muted, left-bordered `details`; the muted tone applies to the whole body including headings and code.
- [ ] A subagent's final report — the text result of a tool message that has `subagent` set — renders as markdown under the nested transcript (or in the frame body), not as monospace output. Non-text results keep the JSON tree.
- [ ] **User messages stay literal** (`whitespace-pre-wrap`): what a person typed is shown as typed, so pasted code, underscores and asterisks are never reinterpreted. Stated in `SPEC.md`.
- [ ] Tool results of ordinary tools stay as they are (monospace / JSON tree): they are program output, not prose. MCP tool results likewise.
- [ ] `SystemMessage` and `ResultMessage` text stay plain (short orchestrator- or CLI-generated strings).
- [ ] The frontend chain passes.

## Implementation notes
- Files: `frontend/src/session/messages/ThinkingMessage.tsx`, `frontend/src/session/tools/ToolResult.tsx` or `ToolFrame.tsx`/`SubagentGroup.tsx` (wherever the subagent's result is drawn after `6sbzz` made rows start collapsed), `SPEC.md`.
- `resultText()` in `tools/toolInput.ts` already flattens a text-block result to a string; use it to decide "is prose".
- Redacted thinking (`redacted: true`) has no text to render; keep its placeholder.

## Edge cases
- A subagent report of tens of kilobytes: it is inside a collapsed row by default, so it is parsed only when opened — keep it that way (do not render the body of a closed row).
- The 40-line collapse of long tool results does not apply to a markdown report; give it a max height with scroll, or nothing, but not a line count of rendered markdown.

## Testing
- `Transcript.test.tsx` / a new `ThinkingMessage.test.tsx`: a thinking text with a list renders `li`; a subagent fixture whose report contains a table renders a `table` once its row is opened; a user message containing `*not emphasis*` and a fence shows them literally.
