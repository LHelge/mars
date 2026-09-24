---
id: ebk7u
title: Give the project page a Branches tab and move the git panel off the Sessions tab
status: done
priority: P2
created: "2026-09-24T08:19:30.493380Z"
updated: "2026-09-24T09:07:45.454159744Z"
tags:
  - frontend
  - git
  - docs
  - tests
parent: kc8k3
attempts: 1
---

Part of epic kc8k3. The project's `Sessions` tab (`frontend/src/pages/project/SessionsTab.tsx`) is the launch form, the session table and then, at its bottom, the whole project-wide `GitActionsPanel` (`frontend/src/components/git/GitActionsPanel.tsx`): the session-branch table with ahead/behind and merge, rebase and push per row, plus the `Merge any ref` form used to bring `origin/main` into `main`. Two lists of sessions on one tab, one of them long and mostly leftover refs, is the mess the user reported (2026-09-24). Git work is also project work, not session work: integrating upstream and pushing `main` name no session at all.

Invoke the `/frontend-design` skill before reshaping the UI.

## Scope

1. **New tab.** `branches`, labelled `Branches`, in `PROJECT_TABS` (`pages/project/tabs.ts`) after `board`: `sessions`, `board`, `branches`, `profiles`, `shared-dirs`, `states`, `secrets`. `?tab=branches` is a real link like every other tab. The label matches the git panel's existing `Branches` header and the `branches` help topic.
2. **Move, don't fork.** The tab renders the project form of `GitActionsPanel` (the one with no `sessionId`) and the Sessions tab stops rendering it. The session view keeps its one-row panel behind the header's `branch` toggle, unchanged. The project-page panel's session-title lookup keeps reading `projectQueries.sessions` from the cache, as it does now.
3. **Layout of the tab.** Order it by what the operator does: the integration heads first (the default branch marked, its commit), then bringing upstream in (`Merge any ref`, set up for `origin/<default>` into `<default>`), then the session-branch table. Leave room beside each integration head for the push action vbrmg adds. This task does not add it. No API changes: ahead/behind of an integration head against its upstream is not in `Branch` today, and adding it would be its own task.
4. **Sessions tab.** Keeps the launch form and the session table. Its `Branch` column stays a plain value. Linking it to the Branches tab is optional, and it should not become a second place for git actions.

## Documentation (rule 1)

- SPEC.md, "Frontend", Routes: the tab list and the `?tab=` values gain `branches`. Also update "Frontend", Help, where it names the git panel, if the wording changes.
- ARCHITECTURE.md, "Frontend architecture": the `Routes:` line (`ProjectPage, with tabs for …`) and the "Project page" paragraph if it mentions where the git panel sits.
- `frontend/src/help/branches.md`: "The **Merge any ref** form at the bottom of the project's **Sessions** tab" becomes the Branches tab. Check the other help topics for the same pointer.

## Tests

- `frontend/tests/git.spec.ts`: the header note "There is no branches tab" goes, and the scenarios that open the project-wide panel navigate to `?tab=branches` instead of `?tab=sessions`. Add a scenario that the Sessions tab no longer shows the branch table and the Branches tab does, with its coverage row in `frontend/tests/README.md`.
- Unit: `parseProjectTab("branches")` and the tab strip order, beside the existing tab tests.