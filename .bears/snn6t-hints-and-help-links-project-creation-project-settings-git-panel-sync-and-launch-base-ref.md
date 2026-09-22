---
id: snn6t
title: "Hints and help links: project creation, project settings, git panel, sync and launch base ref"
status: open
priority: P2
created: "2026-09-22T19:08:55.734960957Z"
updated: "2026-09-22T19:08:55.734960957Z"
tags:
  - frontend
depends_on:
  - ujccg
parent: gtbp5
---

Tighten hints and add `help` links (topic ids from the infrastructure task). Keep hints to one accurate sentence; the help section carries the rest. Invoke `/frontend-design` first. Update unit/E2E tests that assert the old strings.

- `pages/projects/ProjectCreateForm.tsx` Credential: say it is also used to push and name the minimum permission (GitHub Contents read-and-write; GitLab read_/write_repository), help → `git-credential`. Remote URL: "HTTPS only; no token in the URL." Default branch: help → `branches`.
- `pages/ProjectsPage.tsx` empty state: add that sessions also need an agent credential, help → `getting-started`.
- `pages/project/ProjectSettingsForm.tsx`: Max attempts — "How many times agents may pick up a task in one state before it escalates to the human state (1–20)", help → `task-flow`. Default branch — replace "integration head" jargon: the Mars branch sessions start from and merges target by default, help → `branches`. Unattended fieldset: help → `automation`.
- `pages/project/ProjectHeader.tsx` "Fetch now": title/hint that a fetch updates `origin/*`, never `main`.
- `session/SessionActions.tsx` / `SessionHeader.tsx` Sync: "Copy this session's commits into the project mirror (uncommitted changes are not included)."
- `components/git/*`: Branches section help → `branches`; MergeForm note that a branch merge grants no task approval; RebaseForm hint about a running session's work tree; PushForm remote branch hint "Branch name on the remote; pushed with the project's git credential."
- `launch/BaseRefSelect.tsx` (and so LaunchSessionForm / tasks/LaunchForTask): hint "Where the session's branch starts; pick a session branch to continue earlier work", help → `branches`.