---
id: agyg2
title: "A Mars-owned session preamble tells every agent the git flow: commit on session/<sid>, Mars fetches it back, nothing else is the agent's"
status: done
priority: P2
created: "2026-09-26T20:26:00.089293231Z"
updated: "2026-09-26T21:09:23.419441528Z"
tags:
  - orchestrator
  - agent
  - sessions
  - git
  - docs
attempts: 1
---

## Summary
Sessions regularly try to do more with git than commit to their own branch: check out or merge into `main`, create other branches, push. The seeded prompts give role-specific git guidance (implementer, reviewer, merger, resolver), but the default `claude` profile and the planner say nothing about git, no prompt states the flow as a whole, and prompts are copied into a project at creation, so fixing the templates reaches neither existing projects nor profiles a user wrote.

Decided with the user: the orchestrator composes one fixed, Mars-owned paragraph into every session's system prompt at launch, **before** the profile's own prompt, whatever the profile, project or backend. Templates stay as they are.

## Documents
- `ARCHITECTURE.md`, "Git model" → "Session clone", "Fetch-back", "Merge, rebase, push" (the facts the preamble states); "Launch sequence" (where it is composed).
- `SPEC.md`, "Role profile templates" / "Agent profiles" (a profile's system prompt is what the user edits; the preamble is added to it and is not editable) — reproduce the preamble text verbatim in `SPEC.md`, the way the templates are reproduced, and test that the two agree (as `tests/profile_templates.rs` does for the templates).
- New ADR in `docs/decisions/` (next free number): a Mars-owned launch preamble rather than a paragraph in every template. Rejected alternatives: the paragraph in each template (copied, not referenced — reaches only new projects and seeded profiles), and both.

## What the preamble says (facts to state, wording is yours; keep it short — a paragraph, not an essay)
- The working tree is a clone of the project repository on the branch `session/<sid>` (use the real session id; the branch name is known at launch), started from the base the session was launched from.
- Committing on that branch is the whole of the agent's git job. Mars fetches the branch back by itself (at session end, on a sync, before a hand-off, merge, rebase or push). Uncommitted work is not picked up; work on another local branch is never fetched back.
- There is no push: the container has no credential and the project repository is read-only from inside it. Do not check out, merge into, reset or push `main` or any other branch as a way of delivering work; getting work onto an integration branch or upstream is done by Mars — by the task flow (hand-off, review, merge) or by a person from the UI.
- `origin` is the project repository: fetching from it and rebasing or merging into the agent's own branch is fine and expected. Its remote-tracking branches are Mars's integration heads; upstream-tracking and session refs need an explicit refspec.
- Nothing product-specific about a task tracker (the preamble is about git only).

## Acceptance criteria
- [ ] The composition happens in one place in the orchestrator, backend-agnostic (the `LaunchContext` the launcher builds in `session/launcher.rs`, not inside the Claude adapter), for fresh launches and resumes, conversational and ephemeral, and for a profile with an empty system prompt (then the preamble alone is sent).
- [ ] The preamble text lives beside the templates as a Markdown file embedded with `include_str!`, with the session id substituted at launch; `SPEC.md` reproduces it verbatim and a test compares them.
- [ ] Unit tests: preamble + profile prompt order and separation; empty/blank profile prompt; the session id appears in the branch name. The Claude adapter's argv tests still pass unchanged or are adjusted only where they assert the final prompt.
- [ ] The frontend profile editor's system-prompt field says, as its hint, that Mars adds a fixed note on the session's git setup before this prompt (one sentence, through `FieldShell`'s hint; help topic if one fits). E2E coverage row only if the coverage table requires it.
- [ ] Existing seeded templates are not edited in this task; any sentence in them that now contradicts the preamble is reported, not changed.
- [ ] Both quality chains pass.

## Out of scope
- Rewriting existing projects' stored profile prompts.
- Enforcing the flow technically (it already is: no push, RO mirror); this is about telling the agent.