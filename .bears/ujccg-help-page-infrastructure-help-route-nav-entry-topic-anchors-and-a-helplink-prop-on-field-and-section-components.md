---
id: ujccg
title: "Help page infrastructure: /help route, nav entry, topic anchors and a HelpLink prop on field and section components"
status: done
priority: P2
created: "2026-09-22T19:08:05.750060375Z"
updated: "2026-09-22T20:30:24.683436716Z"
tags:
  - frontend
parent: gtbp5
attempts: 1
---

Implements the epic's shape (`SPEC.md`, "Frontend" — add the route there; `CLAUDE.md` frontend conventions — add `HelpLink` to the shared UI list). Invoke `/frontend-design` first.

- Route `/help` (signed-in, `App.tsx` before the `*` route) rendering a `HelpPage`: a table of contents and one section per topic, each with a stable `id` anchor; navigating to `/help#<topic>` scrolls to it.
- Topics are a closed set: a `const HELP_TOPICS` array and derived `HelpTopic` union in `src/help/` (per the types convention), with ids for: `getting-started`, `git-credential`, `agent-credentials`, `secrets`, `profiles`, `skills`, `automation`, `branches`, `task-flow`, `shared-directories`. Content tasks fill the sections; this task ships each with a heading and a placeholder paragraph so link tasks can target them.
- Content format: one Markdown file per topic under `src/help/`, imported `?raw` and rendered with the existing `Markdown` component (react-markdown + remark-gfm, tables needed). Internal links in help prose go through a `helpPath(topic)` helper (like `taskPath`), not hand-built strings.
- `HelpLink({ topic, children? })` — a small "Learn more" router link to `helpPath(topic)`. Add an optional `help?: HelpTopic` prop to `FieldShell` (and so `FormField`), `SectionHeader`, `EmptyState` and the profile editor's `Fieldset`, rendering a `HelpLink` beside — not inside — the hint/description, so `aria-describedby` still points at the hint text only.
- Nav: a "Help" entry in `PageLayout`'s `NAV` (or an icon link in the right cluster — pick per frontend-design).
- Tests: unit tests for `helpPath`/topic parsing and FieldShell's `help` rendering (describedby unchanged); an E2E scenario that follows a HelpLink to its anchored section; add the row to `frontend/tests/README.md` and any `data-testid` to `utils/testIds.ts`.