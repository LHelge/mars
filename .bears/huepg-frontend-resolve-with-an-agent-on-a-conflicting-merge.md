---
id: huepg
title: "Frontend: \"Resolve with an agent\" on a conflicting merge"
status: open
priority: P2
created: "2026-09-26T08:44:27.857677911Z"
updated: "2026-09-26T08:44:27.857677911Z"
tags:
  - frontend
  - git
  - sessions
  - docs
parent: e3gpb
---

## Summary

When a merge started from the git panel (the Branches tab: `Merge any ref` and the session-branch rows; the session header's branch panel is the same component) answers 422 with `conflicts`, the panel lists the paths as it does now, and adds a **Resolve with an agent** action.

The action opens an inline form, owned by `useFormSubmit`, with:
- a conversational profile select. It preselects a profile named `resolver` if the project has one, otherwise the project's default profile, and lists only conversational profiles;
- a preview of the generated first message, which can be edited.

The form launches `POST /projects/{pid}/sessions` with:
- `base_ref` = the merge **target**;
- `message` = the generated text;
- no task.

On success it navigates to the new session. Because it leaves the screen, its invalidations are started but not awaited, following CLAUDE.md's launch-form exception, with the reason written at the call site.

## The generated message

Plain text, built by one pure helper, for example `components/git/resolveMessage.ts` with a Vitest beside it:

```
Merge <source api name> (<full commit>) into your branch, which starts at <target> (<short target commit>).
The merge Mars tried conflicted in:
- <path>
- ...
Fetch the source with: git fetch origin <refspec>
Resolve the conflicts, run the project's checks, commit the merge, and tell me when your branch is ready to merge into <target>. Do not merge into <target> yourself.
```

- The refspec is `refs/sessions/<sid>` for a session source, `refs/handoffs/<id>` for a hand-off, and the branch name for an integration head.
- The source commit comes from the panel's already-loaded session-branch row, or from the head listing. If the panel cannot name it, leave the commit out rather than guess.

## Notes

- The profile list comes from the cached profiles query the launch form uses. Nothing new on the backend.
- If the project has no conversational profile, say so and link to the profiles tab. Do not show a disabled button without a reason.
- Follow CLAUDE.md frontend conventions: `FieldShell` over the select and textarea, `SubmitButton`, `Icon.*` from `icons.ts`, a `data-testid` constant, and a route helper for the session path. Invoke `/frontend-design` first.

## Docs (same commit)

- SPEC.md "User-facing features" → Git operations: one sentence.
- SPEC.md "Frontend": the git panel conflict answer.
- In-app help `frontend/src/help/branches.md`: a short "Resolving a conflict" section.
- A row in the `frontend/tests/README.md` coverage table.

## Testing

- Vitest: the message helper for each source kind and for a missing commit; profile preselection.
- Playwright: arrange a conflicting merge through the API (two sessions editing the same file, the first merged), trigger the merge from the UI, and check the conflict list and the action. Launch with the default profile and assert that the new session's first user message names the source ref and the conflicting path, and that its base is the target. The stub image does not resolve anything, and nothing more is needed.