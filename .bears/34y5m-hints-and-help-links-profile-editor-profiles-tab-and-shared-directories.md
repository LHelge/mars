---
id: "34y5m"
title: "Hints and help links: profile editor, profiles tab and shared directories"
status: open
priority: P2
created: "2026-09-22T19:08:55.767960602Z"
updated: "2026-09-22T19:08:55.767960602Z"
tags:
  - frontend
depends_on:
  - ujccg
parent: gtbp5
---

Tighten hints and add `help` links. Invoke `/frontend-design` first. Update tests that assert old strings.

`pages/project/ProfileEditor.tsx`:
- Header: help → `profiles`. Ephemeral kind hint: "…the only kind that can run automatically or on a schedule."
- Model: "A Claude Code model alias or full model id; empty uses the CLI default."
- Image: must be built FROM the Mars session image; sessions cannot install system packages; help → `profiles`. Runtime: append "…must be configured in the container engine."
- **Idle timeout is inaccurate for ephemeral profiles**: the reaper fails those as `stalled`, it does not park them (ARCHITECTURE "Session owner task", "Stop semantics"). Make the hint kind-dependent and say a long silent command counts as idle.
- Unattended fieldset: "Project settings can pause this or cap the whole project", help → `automation`. Schedule cron: "Missed or refused runs are skipped, not retried."
- System prompt: "Describes this agent's job; repository conventions come from its CLAUDE.md and skills from .claude/skills", help → `skills`.
- Git tools: one line per tool, notably `push` publishes a branch to the remote with the project's git credential and task `merge` needs an approved hand-off.
- `ServedStatesFieldset.tsx`: what served states mean for a conversational profile (what `ready` offers) vs an auto-launch one (what the dispatcher watches). `SecretsFieldset.tsx`: orchestrator-only secrets are skipped even when declared.
- `ProfilesTab.tsx`: section help → `profiles`; make the default-profile marker's meaning (pre-selected at launch, cannot be deleted) visible, not tooltip-only.

`pages/project/SharedDirsTab.tsx` / `SharedDirForm.tsx`: why share (one cache instead of one per session) and what not to share, help → `shared-directories`; Name format hint (`a-z0-9`, `_`, `-`, 1–64); Container path constraints hint; blocked Clear/Remove reason visible, not tooltip-only.