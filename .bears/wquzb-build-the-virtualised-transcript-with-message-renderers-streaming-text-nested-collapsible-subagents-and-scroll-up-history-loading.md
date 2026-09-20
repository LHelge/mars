---
id: wquzb
title: Build the virtualised Transcript with message renderers, streaming text, nested collapsible subagents and scroll-up history loading
status: in_progress
priority: P1
created: "2026-09-16T20:43:53.507147855Z"
updated: "2026-09-20T07:49:40.234427486Z"
tags:
  - frontend
  - sessions
depends_on:
  - bxhas
parent: cgdc2
attempts: 1
---

## Summary
Build the `Transcript` component: a `@tanstack/react-virtual` list over `store.order` with dynamic row measurement, stick-to-bottom while streaming, older-history loading when the user scrolls to the top, and one renderer per non-tool message kind (user, assistant markdown with a streaming cursor, thinking, system, result, raw). Tool messages get a generic frame here (name, running spinner, error tint, collapsible body) plus the nested collapsible subagent transcript; the per-tool-family renderers are the next task and plug into the frame through a registry.

## Documents
- `SPEC.md` "Frontend" stack (`@tanstack/react-virtual` for the transcript, `react-markdown` for text); "Transcript rendering" (markdown for assistant text; a nested, collapsible transcript for subagents; a JSON tree for anything else and for `raw`; long tool results collapsed above 40 lines); "Session state" (`order`, `messages`, `subagents`, events with `parent_tool_use_id` nested under the tool message).
- `SPEC.md` "User-facing features", Sessions paragraph (full transcript with per-tool rendering and nested subagents; anyone opening a session later sees everything that happened).
- `SPEC.md` "AgentEvent" (`thinking.redacted` reflects backend redaction; `result` fields; `state_change.signal` so the UI can say "stopped" versus "killed").
- `ARCHITECTURE.md` "Event delivery" (older history fetched on scroll-up with `seq` cursor).
- `CLAUDE.md` "Frontend conventions" (operator console: monospace where content is code or logs, quiet colour for state; invoke `/frontend-design`).

## Acceptance criteria
- [ ] `frontend/src/session/Transcript.tsx` renders `store.order` through `useVirtualizer` with `measureElement` (rows have variable height), `overscan` 8, and a scroll container that fills the available height.
- [ ] Auto-follow: when the user is within 48 px of the bottom, new messages and `text_delta` growth keep the view pinned to the bottom; otherwise a `Jump to latest` pill appears with the count of new top-level messages since the user scrolled up.
- [ ] Scrolling within 200 px of the top while `hasMore` is true calls `loadOlder()` once per page and preserves the visual scroll position after the prepend (adjust `scrollTop` by the height delta measured after render).
- [ ] Renderers in `frontend/src/session/messages/`: `UserMessage` (right-aligned block, `pending` dimmed with "sending…", `rejected` red with the reason and a `Resend` affordance that calls a prop callback), `AssistantText` (`react-markdown`, code blocks monospace; a blinking cursor while `streaming`), `ThinkingMessage` (collapsed by default, header `Thinking` or `Thinking (redacted by backend)`), `SystemMessage` (levels `info` grey, `warn` amber, `error` red; `detail` rendered in a `<details>`), `ResultMessage` (subtype, turns, duration, `cost_usd` to 4 decimals when present, error tint when `is_error`), `RawMessage` (monospace pretty-printed JSON until the JSON tree from the next task replaces it).
- [ ] `ToolFrame` in `frontend/src/session/tools/ToolFrame.tsx`: header with tool name, running spinner or done/error mark, `truncated` badge; body slot chosen by `toolRendererFor(name)` from `frontend/src/session/tools/registry.ts` (this task ships a `DefaultToolRenderer` showing input and result as pretty JSON; the next task fills the registry).
- [ ] `SubagentGroup`: a tool message with `subagent` renders its `description`/`agent_type` header and, expanded, a non-virtualised nested list of `subagents[tool_use_id]` using the same renderers (recursively, depth is unbounded but rendered plain); collapsed by default once the subagent has ended, expanded while running.
- [ ] Every row is keyed by message id; a `text` replacing a streaming message does not remount the row (same id).
- [ ] Markdown rendering never executes HTML (`react-markdown` default, no `rehype-raw`); links open in a new tab with `rel="noopener noreferrer"`.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/session/Transcript.tsx`, `frontend/src/session/messages/{UserMessage,AssistantText,ThinkingMessage,SystemMessage,ResultMessage,RawMessage,MessageRow}.tsx`, `frontend/src/session/tools/{ToolFrame,DefaultToolRenderer,registry}.tsx|ts`, `frontend/src/session/SubagentGroup.tsx`, `frontend/src/session/index.ts`.
- Props: `Transcript({ sessionId, loadOlder, onResend })`; it reads the store through `useSessionStore(sessionId, selector)` with fine-grained selectors (`order` array identity, and per-row `messages[id]`) so a delta re-renders one row.
- Registry contract for the next task: `registerToolRenderer(matcher: (name: string) => boolean, component: ToolRenderer)` and `toolRendererFor(name)` returning the first match or `DefaultToolRenderer`; `ToolRenderer = FC<{ message: ToolMessage }>`.
- Use a `useStickToBottom` helper (`frontend/src/session/useStickToBottom.ts`) that tracks the near-bottom flag from `scroll` events and re-pins in a layout effect when `order.length` or the last message's text length changes.
- Keep row chrome minimal: left gutter with a kind glyph, timestamp on hover (`ts` is not on the message; omit or add `ts` to `Message` in the store if wanted; keep the store change small and covered by its tests).

## Edge cases
- Extremely long single assistant messages: rely on virtualizer measurement; do not cap height.
- `order` may shrink to zero on `reset()` (navigating between sessions): the virtualizer must handle `count` going to 0 without errors.
- Prepending while pinned to the bottom must not jump the view to the top.
- `subagents[parent]` referenced by a tool message that is not in `order` (nested subagent inside a subagent): resolved recursively from `messages`, never from `order`.
- A user message with `reply_to` (an answer) is labelled `Answer`.

## Testing
- Vitest with `@testing-library/react` (add as dev dependency in this task if absent): render `Transcript` against a store populated from the `subagent.json` and `simple_turn.json` fixtures with the virtualizer's `scrollElement` mocked to a fixed height; assert the rendered row order, that the nested subagent children render inside the parent, that streaming text shows the cursor, and that `loadOlder` is called once when scrolled to top with `hasMore`.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `LoadingState`, `EmptyState` for the empty transcript, and the operator-console base tokens in `index.css`.

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- No user message carries `reply_to`, so nothing is labelled `Answer`.
