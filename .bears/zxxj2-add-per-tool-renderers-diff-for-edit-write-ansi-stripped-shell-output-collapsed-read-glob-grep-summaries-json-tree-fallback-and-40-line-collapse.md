---
id: zxxj2
title: "Add per-tool renderers: diff for edit/write, ANSI-stripped shell output, collapsed read/glob/grep summaries, JSON tree fallback and 40-line collapse"
status: in_progress
priority: P1
created: "2026-09-16T20:45:51.403793733Z"
updated: "2026-09-20T08:02:56.240050498Z"
tags:
  - frontend
  - sessions
  - tests
depends_on:
  - wquzb
parent: cgdc2
attempts: 1
---

## Summary
Fill the tool renderer registry with one renderer per tool family, chosen by tool name: a line diff for edit and write tools computed from `old_string`/`new_string` or file content, monospace output with ANSI escapes stripped for shell tools, a one-line summary that expands on click for read, glob and grep tools, and a collapsible JSON tree for every other tool, for MCP tools and for `raw` messages. Tool results longer than 40 lines are collapsed with a "show all" control. The diff renderer is shared with the Changes panel task.

## Documents
- `SPEC.md` "Frontend", "Transcript rendering": one renderer per tool family chosen by tool name; side-by-side or unified diff for edit and write tools (computed from `old_string`/`new_string` or file content); monospace with ANSI stripping for shell tools; collapsed one-line summary for read, glob and grep that expands on click; nested collapsible transcript for subagents (previous task); JSON tree for anything else and for `raw`; long tool results collapsed above 40 lines.
- `SPEC.md` "AgentEvent": `tool_call {tool_use_id, name, input}`, `tool_result {content: string | unknown, is_error, truncated}`; `docs/data-model.md` `events` row size (results above 256 KiB are truncated in the payload with `truncated: true`).
- `SPEC.md` "AgentEvent" translation rules (the subagent tool is `Task` or `Agent` depending on CLI version; the frontend must treat both names as the subagent family).
- `CLAUDE.md` "Frontend conventions" (monospace where content is code or logs).

## Acceptance criteria
- [ ] `frontend/src/session/tools/registry.ts` is populated at module load so `toolRendererFor(name)` returns: `EditToolRenderer` for `Edit`, `MultiEdit`, `Write`, `NotebookEdit`; `ShellToolRenderer` for `Bash`; `SummaryToolRenderer` for `Read`, `Glob`, `Grep`, `LS`, `WebFetch`, `WebSearch`; `SubagentGroup` (previous task) for `Task` and `Agent`; `JsonToolRenderer` for everything else including `mcp__*` names and `unknown`. Matching is exact on the name with a case-insensitive fallback.
- [ ] `frontend/src/utils/diff.ts` exports `lineDiff(oldText: string, newText: string): DiffLine[]` (`{type: "context" | "add" | "del", text, oldNo?, newNo?}`) using an LCS over lines (no new dependency), and `parseUnifiedPatch(patch: string): PatchFile[]` (`{path, hunks: {header, lines: DiffLine[]}[]}`) for the Changes panel.
- [ ] `frontend/src/components/DiffView.tsx` renders `DiffLine[]` unified by default with a `Side by side` toggle, line numbers, add/del tinting, and an optional file header; the same component is used by `EditToolRenderer` and later by the Changes panel.
- [ ] `EditToolRenderer`: `Edit` → diff of `input.old_string` vs `input.new_string` under `input.file_path`; `MultiEdit` → one diff block per `input.edits[]`; `Write` → all lines of `input.content` as additions under `input.file_path`; `NotebookEdit` → `input.new_source` as additions with the cell id; the result (usually a confirmation string) shown below, error tinted when `is_error`.
- [ ] `ShellToolRenderer`: shows `input.command` (and `input.description` when present) as a prompt line, then the result content through `stripAnsi` from `frontend/src/utils/ansi.ts` in a `<pre>` with horizontal scroll; `is_error` tints the frame.
- [ ] `SummaryToolRenderer`: one-line summary `Read <file_path>[ :offset-limit]`, `Glob <pattern> in <path>`, `Grep <pattern> in <path>`, `LS <path>`, `WebFetch <url>`, `WebSearch <query>`; clicking expands to the input JSON and the result content (monospace).
- [ ] `JsonToolRenderer` and `RawMessage` use `frontend/src/components/JsonTree.tsx`: collapsible object/array nodes (collapsed beyond depth 2 by default), strings truncated at 500 characters with expand, no external dependency.
- [ ] `frontend/src/components/CollapsibleLines.tsx` collapses any result above 40 lines to the first 40 with `Show all N lines` / `Collapse`; applied by every renderer to string results.
- [ ] `truncated: true` results show a notice `Result truncated at 256 KiB; the full output is in the session transcript file`.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/session/tools/{EditToolRenderer,ShellToolRenderer,SummaryToolRenderer,JsonToolRenderer}.tsx`, `frontend/src/session/tools/registry.ts` (extend), `frontend/src/components/{DiffView,JsonTree,CollapsibleLines}.tsx`, `frontend/src/utils/{diff,ansi}.ts` with tests.
- `stripAnsi` regex: `/[][[\]()#;?]*(?:(?:(?:[a-zA-Z\d]*(?:;[-a-zA-Z\d\/#&.:=?%@~_]*)*)?)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g` plus stripping of `\r` before `\n`.
- Tool input fields are unknown at compile time: narrow with small type guards (`isEditInput(input): input is {file_path: string; old_string: string; new_string: string}`) and fall back to `JsonToolRenderer` when the guard fails, never crash on unexpected shapes.
- `lineDiff` complexity is O(n·m) on line counts; cap at 5 000 lines per side and fall back to "old" / "new" blocks without alignment above that, with a notice.
- The side-by-side mode can be P3 polish if time is short; unified is required.

## Edge cases
- `tool_result.content` as an array of `{type: "text", text}` blocks (Claude tool results): join text blocks with newlines before rendering; non-text blocks fall to the JSON tree.
- Binary or extremely long single-line output: `<pre>` with `overflow-x: auto`; do not wrap.
- An `Edit` whose `old_string` is empty renders as a pure insertion.
- Unknown MCP tool names still show the name unmangled (`mcp__bears__create_task`) in the frame header.

## Testing
- Vitest: `diff.test.ts` (insertion, deletion, replacement, identical, empty old, line numbers, `parseUnifiedPatch` on a two-file patch with two hunks), `ansi.test.ts` (colour codes, cursor movement, OSC sequences, `\r\n`), registry test (each listed name resolves to the expected renderer; `mcp__x` → JSON), and one render test per renderer using `@testing-library/react` with a hand-built `ToolMessage`.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- none beyond the previous tasks of this epic.