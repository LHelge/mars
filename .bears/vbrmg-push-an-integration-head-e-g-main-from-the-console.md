---
id: vbrmg
title: Push an integration head (e.g. main) from the console
status: open
priority: P2
created: "2026-09-22T21:38:05.001937561Z"
updated: "2026-09-22T21:38:05.001937561Z"
tags:
  - frontend
---

Found while writing the `branches` help topic (gtbp5 / 4qa8f): the console's `PushForm` appears only on session-branch rows of the git panel, so a user cannot push an integration head such as `main` from the UI — yet README "Operating notes" (git fetch bullet) and the help tell users to merge `origin/main` into `main` and then push, and merged work (the `merger` profile, the task's **Merge approved hand-off**, and later tykeu's auto-merge) waits on the integration head until pushed. Today that push needs the API (`POST /projects/{pid}/git/push`, which already accepts integration heads — `SPEC.md` "Git") or an agent granted the `push` tool, which no starter profile has.

Verify the gap first (`frontend/src/components/git/*`, `GitActionsPanel.tsx`). Then add a push action for integration heads — e.g. beside the default branch in the Branches panel or in the "Merge any ref" area — reusing `PushForm` (remote branch defaulting to the head's name, force-push confirmation, rejection message, GitHub compare link). Update `frontend/src/help/branches.md` "Pushing" (it currently says the console pushes only session branches), `SPEC.md` "Frontend" where it describes the git panel, and add an E2E scenario plus its coverage row in `frontend/tests/README.md`.