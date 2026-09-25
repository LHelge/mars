---
id: hwcn9
title: "Frontend: templates carry auto_launch, help and launch form read right with the new seeded profiles"
status: done
priority: P2
created: "2026-09-25T09:17:27.971344Z"
updated: "2026-09-25T10:18:28.734088Z"
tags:
  - frontend
  - profiles
depends_on:
  - "35xvy"
parent: xz6yq
---

The frontend half of epic `xz6yq` (read it first), except the Playwright suite. The orchestrator task defines the API: `ProfileTemplate` gains `auto_launch` (`SPEC.md`, "Agent profiles").

- `src/types/profiles.ts`: `ProfileTemplate.auto_launch: boolean`. Creating a profile from a template (`pages/project/profileForm.ts` and the profile editor) carries `auto_launch` into the form, so an auto-launched template starts with the toggle on. Without an unattended credential, saving then gets the existing refusal at the toggle, which links to the credential. That is intended; check it reads right.
- The launch form (`pages/project/LaunchSessionForm.tsx`) already preselects the default profile, which is now the conversational `claude`. Check `profileLabel` reads well for a profile that serves no queue ("claude — conversational, serves no queue"), and that the task drawer's `LaunchForTask` still offers a sensible launch for a task in `ready` or `review` now that the profile serving those states is ephemeral ("run once"). Change only what reads wrong.
- In-app help (`src/help/profiles.md`, `getting-started.md`, `task-flow.md`, `automation.md` and any other help that describes the seeded profiles): four seeded profiles, `claude` the default for talking to an agent, implementer and reviewer ephemeral and dispatched automatically once an agent credential is stored at project or global scope (link the agent-credentials help), planner conversational. Field hints naming the default profile, too.
- Unit tests whose fixtures or assertions describe the old seeded set (`LaunchSessionForm.test.tsx`, `ProfilesTab.test.tsx`, `profileForm.test.ts`, `LaunchForTask.test.tsx`): update what describes seeding. Add a test that a template with `auto_launch: true` yields a form with it on.
- Chain: `npm run lint && npx tsc -b && npm run build && npm run test:unit`.