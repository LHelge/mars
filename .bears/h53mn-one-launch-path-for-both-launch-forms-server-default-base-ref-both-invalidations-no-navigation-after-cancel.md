---
id: h53mn
title: "One launch path for both launch forms: server-default base ref, both invalidations, no navigation after Cancel"
status: open
priority: P2
created: "2026-09-21T10:54:23.047352111Z"
updated: "2026-09-21T10:54:23.047352111Z"
tags:
  - frontend
  - technical-review
  - bug
  - refactor
parent: "579dz"
---

Problem: pages/project/LaunchSessionForm.tsx and tasks/LaunchForTask.tsx implement the same launch twice and have diverged. (Both files changed after the review for the agent-credential notice; line numbers below are from before it, the findings were re-checked on main.)
- LaunchSessionForm prefills `baseRef` with project.default_branch and always sends it, although "" already means "server default" and BaseRefSelect's empty option reads "Project default (main)". Change the default branch in ProjectSettingsForm on the same page and the launch form still posts base_ref "main"; while branches load — or permanently if that retry:false query fails — "main" is not a known ref, so the select shows "Custom ref..." with a text box containing main. The prefill is the only reason the useRef + useEffect that clears the base once on a hand-off exists. LaunchForTask starts at "".
- LaunchForTask's Cancel and kind toggles are live during the request (every other form disables Cancel while loading): click "Run once", then Cancel; the panel unmounts, the closure continues, the session is created and navigate('/sessions/...') pulls the user off the board after they cancelled. It also awaits invalidateQueries(detail) before navigating, so the "one click" launch waits for a full refetch of a drawer that is about to unmount.
- Diverged invalidation: LaunchSessionForm invalidates projects.sessions but not the task store or detail; LaunchForTask invalidates the task but not projects.sessions, so the Sessions tab can miss the new session for up to IDLE_POLL_MS (60 s).
- Duplicated: the derived-default profile select, SessionCreateInput assembly with optional spreads, create -> invalidate -> navigate, the message textarea, the hand-off base note, and the short-commit helper (utils/format.ts shortSha vs tasks/launchRules.ts shortCommit). LaunchSessionForm uses BaseRefSelect, LaunchForTask a free-text box.
- The project is fetched by three inline useQuery copies inside the drawer (LaunchForTask twice, MergeTaskAction once) with options that differ from pages/project/useProject.ts (retry:false, cloning poll) on the same key, although BoardTab already holds the loaded Project and passes only its id. listBranches, listProfiles and listProjectSessions are likewise spelled inline in 3-5 files each.

Acceptance: a shared `useLaunchSession(projectId)` does create, both invalidations and the navigation, and does not navigate when the form that started it is gone; Cancel and the toggles are disabled while it runs. Both forms start the base at "" and send base_ref only when the user chose one; the clear-once effect and ref are deleted. Shared ProfileSelect and hand-off base note; BaseRefSelect in both; one short-commit helper. The project is passed down to the drawer or read through one queryOptions factory beside queryKeys, and the other repeated query spellings move to factories too. Tests: default-branch change is respected by a launch without a chosen base; Cancel mid-request does not navigate; both caches are invalidated from either form. E2E coverage table updated if a scenario is added.

References: frontend/src/pages/project/LaunchSessionForm.tsx, BaseRefSelect.tsx, useProject.ts; frontend/src/tasks/LaunchForTask.tsx, MergeTaskAction.tsx, launchRules.ts; frontend/src/utils/format.ts; frontend/src/services/queryKeys.ts. Contract: SPEC.md, "Sessions" (POST, base_ref default) and "Frontend", launch forms.