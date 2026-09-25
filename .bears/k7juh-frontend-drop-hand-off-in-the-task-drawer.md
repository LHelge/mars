---
id: k7juh
title: "Frontend: drop hand-off in the task drawer"
status: open
priority: P2
created: "2026-09-25T21:04:13.088338462Z"
updated: "2026-09-25T21:04:13.088338462Z"
tags:
  - frontend
  - tracker
  - docs
depends_on:
  - x4st6
parent: ny9yq
---

## Summary

Add a "Drop hand-off" action to the task drawer's hand-off section, shown only when the task has a current hand-off. It uses the endpoint from task x4st6. The action is inline, with a `ConfirmPanel` that holds a required comment field, so it counts as a form: use `useFormSubmit`. The confirm label names the target, for example `Drop hand-off of #42`. The panel explains that the next launch for this task will start from the default branch, and that the hand-off stays in the task's history. After it succeeds, it settles through `useSettleTask` (`tasks/taskWrites.ts`), with a per-verb write hook beside the existing ones there.

## Notes

- The service goes in `src/services/`, built with `seg()`. Add a `data-testid` constant in `src/utils/testIds.ts`. Invoke `/frontend-design` first.

## Docs (same commit)

- SPEC.md "Frontend" task board / hand-off controls.
- A row in the `frontend/tests/README.md` coverage table, and a Playwright scenario that publishes a hand-off through the API, drops it through the UI, and asserts the drawer no longer shows a current hand-off.

## Testing

Run the full frontend chain.