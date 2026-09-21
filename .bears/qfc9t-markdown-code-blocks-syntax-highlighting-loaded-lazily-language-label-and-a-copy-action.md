---
id: qfc9t
title: "Markdown code blocks: syntax highlighting loaded lazily, language label and a copy action"
status: done
priority: P2
created: "2026-09-20T23:42:59.889684284Z"
updated: "2026-09-21T14:25:15.117027915Z"
tags:
  - frontend
  - markdown
depends_on:
  - tvcf5
parent: qprj6
attempts: 1
---

## Summary
Agents answer with code all the time. Fenced code blocks get syntax highlighting in the console's palette, a small language label and a `Copy` action — without the highlighter entering the entry bundle or blocking first paint of a message.

## Documents
- `SPEC.md` "Frontend": "Markdown" paragraph (code blocks), "Code splitting" (the highlighter is a lazy chunk, like `xterm.js`)
- The epic's ADR (one sentence if the library choice had a real alternative)
- `/frontend-design` before shaping the block header

## Acceptance criteria
- [ ] A `CodeBlock` component used by `MarkdownBody`'s `pre`: header row with the fence's language (when given) and a `Copy` button; body in the existing bordered monospace style with horizontal scroll.
- [ ] Highlighting through `lowlight`/`highlight.js` core with an explicit language subset registered (at least: rust, typescript/tsx, javascript, json, bash/shell, toml, yaml, sql, python, go, diff, html/xml, css, markdown, dockerfile). No auto-detection for fences without a language — they stay plain, which is cheap and never wrong.
- [ ] The highlighter is a **lazy chunk**: the block renders as plain text immediately and is upgraded when the chunk has loaded; no layout shift (same font, same line height), no `Suspense` fallback flash per block. `npm run build` shows it as its own chunk and the entry chunk does not grow.
- [ ] Token colours are defined once as CSS against the console tokens in `src/index.css` (muted, low-saturation; colour stays reserved for state, so no red/green that reads as failed/running). Works in the dark theme the app ships.
- [ ] `Copy` writes the raw code (no header, no trailing newline added or removed), shows `Copied` only after the clipboard write resolves, and falls back to selecting the text when clipboard access is denied — same rule as `CopyLinkButton`; reuse its logic rather than duplicating it.
- [ ] Blocks above a size limit (say 2,000 lines or 200 KiB) are not highlighted, to keep a pasted log from freezing the tab.
- [ ] The frontend chain passes.

## Implementation notes
- Files: new `frontend/src/components/CodeBlock.tsx` (+ test), `frontend/src/components/highlight.ts` (the lazy loader and language registration), `Markdown.tsx`, `src/index.css`, `package.json`.
- Import `CodeBlock` by path from `Markdown.tsx`; keep lazy-only modules out of the components barrel (see `.claude/agents/task-implementer.md`).
- The diff renderer (`session/` `DiffBody`) has its own colouring for edits; do not try to unify in this task.
- Shell tool output (`ShellToolRenderer`) is not markdown and stays as it is.

## Edge cases
- A fence that is still open while streaming: highlight only when the fence has closed (or debounce), so the highlighter does not run on every delta — coordinate with the streaming task if it has landed.
- Unknown language tag: label shows it, body stays plain.
- Language aliases (`sh`, `zsh`, `ts`, `rs`, `yml`, `console`): map the common ones.

## Testing
- Vitest: plain render before the chunk resolves, highlighted spans after; unknown language stays plain; copy writes the exact source; oversize block is not highlighted.
