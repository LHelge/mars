---
id: kvtxs
title: "Streaming markdown: parse only the growing tail, coalesce deltas per frame, no flicker on half-written constructs"
status: done
priority: P2
created: "2026-09-20T23:43:31.793867486Z"
updated: "2026-09-21T14:25:17.630106376Z"
tags:
  - frontend
  - session
  - markdown
  - perf
depends_on:
  - tvcf5
parent: qprj6
attempts: 1
---

## Summary
While an assistant message streams, every `text_delta` changes `message.text` and `MarkdownBody` re-parses and re-renders the whole message. With GFM, tables and highlighting that cost grows with the message, exactly while the user is watching. Make streaming cost proportional to the unfinished tail, and make half-written constructs (an open fence, a table with only its header row) look calm rather than flickering between interpretations.

## Documents
- `SPEC.md` "Frontend": "Session state" (`text_delta` folding) and the "Markdown" paragraph (streaming behaviour)

## Acceptance criteria
- [ ] A `StreamingMarkdown` wrapper used by `AssistantText` while `message.streaming` is true: the text is split at top-level block boundaries (blank line outside a fence); completed blocks are rendered through memoised `MarkdownBody` instances keyed by index and content, and only the last, growing block re-parses on a delta. When streaming ends, the final `text` event's content renders through the same path, so the DOM does not get rebuilt at the end (no scroll jump, identity kept — the existing test on the streaming cursor's element identity must keep passing).
- [ ] Deltas are coalesced to at most one render per animation frame (the store may already batch; measure first and do this where it is cheapest — in the component or in `sessionStore`'s fold — without ever dropping text).
- [ ] An unclosed code fence renders as a code block from the fence to the end of the text (not as paragraphs that later collapse into a block); highlighting waits for the closing fence.
- [ ] A table whose delimiter row has not arrived yet is shown as text and turns into a table once; it does not alternate.
- [ ] Reference-style links and footnotes, which need the whole document, are allowed to resolve only when the message completes; say so in `SPEC.md`.
- [ ] A measurement in the commit message: render time per delta for a 20 KB message before and after (React Profiler or `performance.now()` around the fold in a Vitest benchmark).
- [ ] The frontend chain passes.

## Implementation notes
- Files: new `frontend/src/components/StreamingMarkdown.tsx` (+ test), `frontend/src/session/messages/AssistantText.tsx`, possibly `frontend/src/session/sessionStore.ts`.
- Block splitting must track fence state (``` and ~~~, with their lengths) so a blank line inside a code block does not split it; lists and blockquotes containing blank lines are the hard case — when in doubt keep more text in the tail rather than splitting wrongly. A wrong split is a rendering bug; a late split is only slower.
- `useStickToBottom` and the virtualiser re-measure on height changes; verify sticking to the bottom still works while a table grows.
- Never keep raw event arrays in state (CLAUDE.md): this task changes how one folded string is rendered, not what the store holds.

## Edge cases
- A delta that ends in the middle of a multi-byte character cannot happen (deltas are strings), but one that ends between `*` and `*` can: the tail re-parse handles it; make sure completed blocks are only frozen after a blank line *followed by* further text.
- Loose lists (items separated by blank lines) must stay one list.
- Very long single paragraphs (no blank line for kilobytes): falls back to today's behaviour; acceptable.

## Testing
- Vitest: feeding the kitchen-sink fixture character by character ends in the same DOM as rendering the full text at once, and no intermediate prefix throws or renders raw HTML elements; completed blocks' DOM nodes keep their identity across deltas; an open fence renders one `pre`.
