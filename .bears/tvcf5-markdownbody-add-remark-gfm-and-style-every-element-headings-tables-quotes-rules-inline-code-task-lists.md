---
id: tvcf5
title: "MarkdownBody: add remark-gfm and style every element (headings, tables, quotes, rules, inline code, task lists)"
status: open
priority: P1
created: "2026-09-20T23:42:39.268671297Z"
updated: "2026-09-20T23:42:39.268671297Z"
tags:
  - frontend
  - markdown
parent: qprj6
---

## Summary
The core of the epic and the part users notice: make `MarkdownBody` render GitHub-flavoured markdown and give every element a style that fits the operator console. One component, used by assistant text, task descriptions, task comments and hand-off comments, so all of them improve at once.

## Documents
- `SPEC.md` "Frontend": stack line; new "Markdown" paragraph (what is rendered, what is not, where `MarkdownBody` is used); "Transcript rendering"
- New ADR (next free number — check `docs/decisions/`; numbers may have been taken by other epics): "Markdown is rendered in the browser from text; no server-side HTML". Context, decision and the rejected `pulldown_cmark` alternatives are in the epic body.
- `CLAUDE.md` "Frontend conventions" stack line (`react-markdown` + `remark-gfm`)
- Invoke `/frontend-design` first (CLAUDE.md): dense, dark-friendly, monospace for code, colour reserved for state — markdown styling uses the neutral console tokens, not new colours.

## Acceptance criteria
- [ ] `remark-gfm` added (`npm install remark-gfm`, pinned like the rest) and passed to `react-markdown` in `frontend/src/components/Markdown.tsx`. Still **no `rehype-raw`**: raw HTML in the text is shown inert (as text or dropped), never parsed into elements.
- [ ] Styled components for: `h1`–`h6` (a compact scale — this is a console, not an article: h1/h2 only slightly larger than body, weight and spacing carry the hierarchy), `blockquote` (left border, muted), `hr`, `table`/`thead`/`th`/`td` (bordered, compact, header weight, column alignment from GFM honoured, **wrapped in a horizontally scrolling container** so a wide table never widens the transcript), `strong`, `em`, `del`, inline `code` (subtle background, no wrap-breaking of short tokens), nested lists (indentation and marker style per depth), task-list items (read-only checkboxes, no bullet), `a` (existing behaviour kept).
- [ ] Inline `code` and fenced `pre > code` are distinguished correctly (react-markdown 10 no longer passes `inline`): the `pre` component owns block styling and the `code` component only adds the inline background when it is not inside a `pre`.
- [ ] **Images are not loaded.** `img` renders as a link with the alt text (`[image: alt]` → href), because fetching a remote image from agent-written text leaks the viewer's address and session timing to whoever the agent was induced to name. State this in `SPEC.md`.
- [ ] Links keep `target="_blank" rel="noopener noreferrer"`; `react-markdown`'s default `urlTransform` (which drops `javascript:` and similar) is left in place, and a test pins that.
- [ ] Long unbroken strings (URLs, hashes, paths) wrap instead of overflowing (`overflow-wrap: anywhere` on prose, not inside `pre`).
- [ ] A fixture `frontend/src/components/fixtures/markdown-kitchen-sink.md` exercising every construct, used by the tests below and handy for eyeballing.
- [ ] `npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e` pass; note the size change of the `project` and `SessionPage` chunks in the commit message.

## Implementation notes
- Files: `frontend/src/components/Markdown.tsx` (+ new `Markdown.test.tsx`), the fixture, `frontend/package.json`, `SPEC.md`, the ADR, `docs/decisions/README.md`.
- `AssistantText` wraps the body in `max-w-prose`; tables and code blocks should be allowed to use the full row width — check whether the width cap belongs on paragraphs rather than on the container.
- The transcript is virtualised and rows are measured from the DOM: a table or heading changes row height, which the virtualiser already handles through measurement, but verify there is no jump when a streaming message turns from raw pipe text into a table.
- Keep the `COMPONENTS` map a module constant (as now) so `react-markdown` does not see a new object per render.

## Edge cases
- A table with one very long cell; a table inside a list item; a code fence inside a blockquote.
- Text that merely looks like markdown in task comments written by people (`snake_case_names`, `2 * 3 * 4`): GFM does not italicise intraword underscores; add a test so it stays that way.
- Footnotes (`remark-gfm` enables them): style minimally or leave default, but make sure the generated `#user-content-fn-…` anchors do not navigate the single-page app away — test one.

## Testing
- `Markdown.test.tsx`: table renders `table`/`th`/`td`; task list renders disabled checkboxes; `~~x~~` renders `del`; `<script>` and `<img onerror>` in the source produce no such elements; `![a](http://x/y.png)` produces no `img`; `[x](javascript:alert(1))` has no `javascript:` href; heading levels map to `h1`–`h6`; intraword underscores stay literal.
