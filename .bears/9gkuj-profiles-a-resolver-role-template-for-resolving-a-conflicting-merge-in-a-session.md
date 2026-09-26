---
id: "9gkuj"
title: "Profiles: a `resolver` role template for resolving a conflicting merge in a session"
status: done
priority: P2
created: "2026-09-26T08:44:27.817657012Z"
updated: "2026-09-26T09:40:59.512336881Z"
tags:
  - orchestrator
  - profiles
  - docs
parent: e3gpb
attempts: 1
---

## Summary

Add a `resolver` role template, offered by `GET /profile-templates` and not seeded into new projects (see the epic body). Follow how `merger` and `tech-debt-scanner` are offered: `orchestrator/src/projects/templates/`, the template list, and SPEC.md "Role profile templates".

## The template

The profile is conversational, serves no task state, and has no git MCP tools. It uses the default image and permission mode, like `claude`. Read `claude.md` and `merger.md` and match their voice. The prompt says, in substance:

- You are launched to bring one branch into your own. Your checkout starts at the merge **target**. The first message names the **source** ref, its commit, and the paths that conflicted when Mars tried the merge.
- Fetch the source with an explicit refspec. A session branch is `git fetch origin refs/sessions/<sid>`, a hand-off is `refs/handoffs/<id>`, and an integration head is an ordinary branch of `origin`. Check that the fetched tip is the named commit.
- Merge it with `git merge`, not a rebase, so the source commit keeps its id. Resolve every conflict by reading both sides and the project's documentation. Never pick one side wholesale unless that is clearly right, and say so.
- Run the project's checks as `CLAUDE.md` or the README describe them. Install what is missing, reusing the installation paragraph the other templates share.
- Commit the merge. Do not merge into the target yourself and do not push: tell the person that your session branch is ready to merge into the target, list what you decided for each conflicted path, and stop.
- If a conflict needs a decision you cannot make from the code and documents, ask the person in the conversation. There is no task to escalate.

## Docs (same commit)

- SPEC.md "Role profile templates": the template, its settings, and that it is offered, not seeded.
- README or in-app help, if templates are listed there (`frontend/src/help/profiles.md`).

## Testing

- Extend the existing profile-templates tests: the listing includes `resolver` in the documented order, with the documented settings (conversational, no served states, no git tools).
- Run fmt, both clippies, and the profile template test binaries.