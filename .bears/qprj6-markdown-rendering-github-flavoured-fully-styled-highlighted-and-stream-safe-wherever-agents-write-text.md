---
id: qprj6
title: "Markdown rendering: GitHub-flavoured, fully styled, highlighted and stream-safe wherever agents write text"
type: epic
status: done
priority: P2
created: "2026-09-20T23:42:11.284441060Z"
updated: "2026-09-21T14:25:24.646048561Z"
tags:
  - frontend
  - session
  - markdown
---

## Scope
Observed on the live stack on 2026-09-21: agent output in a session often "does not render correctly". The cause is not a missing renderer — `components/Markdown.tsx` (`MarkdownBody`, `react-markdown` 10) already renders assistant text, task descriptions and comments — but what it lacks:

- **No GitHub-flavoured markdown** (`remark-gfm` is not installed): tables, task lists, strikethrough and bare-URL autolinks come out as raw text. Agents write tables constantly.
- **Most elements are unstyled.** Tailwind's preflight resets headings, `blockquote`, `hr` and tables to plain text, and `MarkdownBody` only styles `a`, `code`, `pre`, `ul`, `ol`, `p`. A `## Heading` looks like a paragraph; inline code has no background.
- **Code blocks** have no highlighting, no language label and no copy action.
- **Some agent text is not markdown at all**: thinking text and a subagent's final report are drawn as plain pre-wrapped text.
- **Streaming**: the whole message is re-parsed on every `text_delta`.

## Decision taken with the user
Fix it in the frontend. Rendering with the Rust crate `pulldown_cmark` was the original idea and is **rejected**: it would mean rendering HTML in the orchestrator (or shipping WebAssembly), injecting an HTML string into React, and therefore sanitising untrusted agent output (ADR 0027 keeps that output unredacted, and `MarkdownBody` deliberately has no `rehype-raw`); it would need a new field or endpoint for the HTML; and it has no answer for streaming text, which the browser assembles from deltas. The first task records this in an ADR.

## Documents (written by the tasks, rule 1)
- `SPEC.md` "Frontend": stack line (`react-markdown` → with `remark-gfm` and the highlighter), "Transcript rendering", and a new "Markdown" paragraph stating what is rendered, what is deliberately not (raw HTML, remote images), and where
- A new ADR (next free number): markdown is rendered in the browser from text; no server-side HTML
- `CLAUDE.md` "Frontend conventions" stack line if packages are added

## Acceptance criteria
- [ ] A fixture document exercising every construct renders correctly in light of the console theme: headings, emphasis, lists (nested, ordered, task), tables, quotes, rules, links, inline code, fenced code with and without a language, strikethrough.
- [ ] HTML in agent text stays inert and remote images are never fetched.
- [ ] Thinking text and subagent reports are markdown; user messages stay literal.
- [ ] A long streaming answer stays smooth and does not re-parse finished blocks.
- [ ] The frontend quality chain passes; the entry bundle does not grow by the highlighter.
