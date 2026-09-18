---
id: cgdc2
title: Frontend project and session views
type: epic
status: open
priority: P1
created: "2026-09-16T20:14:37.780218089Z"
updated: "2026-09-16T20:15:52.462086216Z"
tags:
  - frontend
  - sessions
depends_on:
  - "2f5u2"
  - h8kw9
---

## Scope

Projects and the live session view.

- `ProjectsPage` (list with clone progress) and `ProjectPage` tabs: sessions (launch from a profile with base ref, first message, optional task; ephemeral "run with a message"), profiles (`ProfileEditorPage`), shared directories (add, remove, clear with running-session refusals surfaced), states (`TaskStatesEditor`, delivered with the board epic but mounted here), secrets tab, git actions (session branches with ahead/behind, merge, rebase, push, conflict display, GitHub compare link after push), fetch now, retry clone, delete.
- `session/`: `sessionStore` (Zustand) folding events into `messages`, `order`, `pendingTools`, `subagents`, `pendingPrompt`, never keeping raw event arrays; `useSessionSocket` with `after = lastSeq`, reconnect with a refreshed token, older history through REST on scroll-up, dedupe on `seq`.
- `SessionView`: virtualised `Transcript` with per-tool renderers (markdown, diff for edit/write, ANSI-stripped monospace for shell, collapsed read/glob/grep summaries, nested collapsible subagents, JSON tree fallback, 40-line collapse), `Composer` (Enter to send, "Interject" mid-turn, answer mode for prompts, hidden for ephemeral, disabled when done/failed), stop/end/sync/retry actions, metadata header with state, branch, container, CLI session id, cost and tokens, `Copy link`, `TerminalView` on `xterm.js` over binary frames, "Changes" panel refreshing on `git` events, task side panel (launched-for task and touched tasks).

## Documents

`SPEC.md` "Frontend" (session state, transcript rendering, changes panel, composer, copy links), "User-facing features" (Projects, Agent profiles, Sessions, Git operations); `ARCHITECTURE.md` "Frontend architecture"; ADR 0022 (not folding raw events).

## Acceptance criteria

- [ ] The store folds every `AgentEvent` kind per the documented rules, unit-tested with fixture event sequences.
- [ ] Reconnect after token expiry resumes from the last `seq` without duplicates.
- [ ] All project-tab actions call their endpoints through `services/` with error states surfaced.
- [ ] Lint, typecheck and build pass.

## Out of scope

Task board and drawer (Frontend task board epic).

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- The epic's scope has no `pendingPrompt` and no composer answer mode.
