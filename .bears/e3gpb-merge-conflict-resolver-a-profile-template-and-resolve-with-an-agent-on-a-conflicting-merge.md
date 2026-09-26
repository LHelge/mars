---
id: e3gpb
title: "Merge conflict resolver: a profile template and \"Resolve with an agent\" on a conflicting merge"
type: epic
status: open
priority: P2
created: "2026-09-26T08:44:06.273256614Z"
updated: "2026-09-26T08:44:06.273256614Z"
tags:
  - orchestrator
  - frontend
  - git
  - profiles
---

## Scope

When a merge of a session branch into an integration head conflicts, Mars today answers 422 with the conflicting paths, and stops there. The user then has to set up the resolution themselves. This happened on the airgap project on 2026-09-26: the docs session's `f626c7b` conflicted with `main` in `CLAUDE.md`.

Everything the resolution needs already works: a session's clone has the Mars repository as `origin`, and can fetch another session's branch with an explicit refspec (`git fetch origin refs/sessions/<sid>`). What is missing is convenience.

1. A `resolver` role template. It is conversational, and its prompt says: fetch the named branch, merge it into your own branch, resolve the conflicts, run the checks, commit, and leave the merge into the target to a person.
2. A "Resolve with an agent" action on a conflicting merge. It launches a session from the merge's **target**, with a generated first message naming the source ref, its commit and the conflicting paths.

## Decisions

- **A person merges the result.** The resolver commits on its own session branch and never merges into an integration head itself. It needs none of the profile-gated git MCP tools. Its branch contains the source commit and the resolution, so it merges cleanly into the target, and the person merges it like any other session branch.
- **Offered, not seeded.** The template is offered through `GET /profile-templates` like `merger` and `tech-debt-scanner`, and is not created in every new project (ADR 0038/0051 pattern). The action therefore works with any conversational profile. It preselects a profile named `resolver` when the project has one, and otherwise the project's default profile.
- **Task hand-offs already have their own path.** An auto-merge conflict sends the task back to its conflict state with the conflicting paths. This epic covers merges a person starts on the Branches tab or in the session branch panel.

## Acceptance Criteria

- [ ] `GET /profile-templates` offers `resolver`, and a project can create a profile from it.
- [ ] A 422 merge conflict in the UI offers "Resolve with an agent". It launches a session from the target, with the generated message, and navigates to that session.
- [ ] SPEC.md "Role profile templates", "User-facing features" → Git operations, and "Frontend" are updated, along with the in-app help and the E2E coverage table.