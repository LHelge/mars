---
id: frhcc
title: "Build the Changes panel: session diff via GET /projects/{pid}/git/diff?head=<session>, refreshed on git events, with file list and patch rendering"
status: open
priority: P2
created: "2026-09-16T20:47:49.074761568Z"
updated: "2026-09-16T20:47:49.074761568Z"
tags:
  - frontend
  - sessions
  - git
depends_on:
  - k97mz
parent: cgdc2
---

## Summary
Add the `Changes` side panel of the session view: when opened it fetches the diff of the session branch against its base through the git diff endpoint, lists the changed files with their status and line counts, and renders the unified patch with the shared `DiffView`. It refetches on every `git` transcript event (tracked by the store's `gitEventSeq`) and on demand, and shows the truncation notice when the patch exceeds 1 MiB.

## Documents
- `SPEC.md` "Frontend", "Changes panel": a tab beside the transcript fetches `GET /projects/{pid}/git/diff?head=<session id>` when opened and again on every `git` event, lists the files with their counts, and renders the patch with the same diff renderer as edit tools; the diff endpoint's internal fetch-back emits no separate `git` event (no refresh loop).
- `SPEC.md` "Git" table and text: `GET /projects/{pid}/git/diff?head=&base=` → `Diff = {base, head, merge_base, files: {path, status, additions, deletions}[], patch, truncated}`; `base` defaults to the project's default branch; a session `head` is synced first; patch truncated above 1 MiB; `head` accepts a session id.
- `SPEC.md` "User-facing features", Sessions paragraph ("Changes" panel with the diff of the session branch against its base).
- `ARCHITECTURE.md` "Git model", Diff paragraph (numstat and diff from merge-base to head against the mirror after fetch-back; no git event for that fetch-back).

## Acceptance criteria
- [ ] `frontend/src/session/ChangesPanel.tsx` registers as the `Changes` side panel, enabled for every session state except `creating`.
- [ ] Query key `["projects", pid, "git", "diff", {head: sessionId, base}]` with `enabled` only while the panel is open; a base selector (default the project's `default_branch`, options from `listBranches(pid)` heads and upstream refs) changes `base`.
- [ ] The store's `gitEventSeq` is observed; a change invalidates the diff query (no polling). A `Refresh` button invalidates manually. The panel header shows `merge_base` and `head` short shas and the time of the last successful fetch.
- [ ] File list: `status` glyph (A/M/D/R…), `path` monospace, `+additions` green / `−deletions` red, totals in the header; clicking a file scrolls to its hunk in the patch.
- [ ] Patch rendered by parsing `patch` with `parseUnifiedPatch` (tool renderers task) into per-file `DiffView` blocks, each collapsible, files above 500 changed lines collapsed by default.
- [ ] `truncated: true` shows the notice `Patch truncated at 1 MiB; file counts are complete`.
- [ ] Empty diff shows `No changes against <base>`; errors (400 unresolvable base, 409/500) show the server message with `Retry`; the previous diff stays visible while refetching (`keepPreviousData`).
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/session/ChangesPanel.tsx`, `frontend/src/session/ChangesFileList.tsx`, registration in `sidePanels.ts`; uses `services/git.ts#getDiff(pid, {head: sessionId}, base)` and `components/DiffView`.
- Subscribe to `gitEventSeq` with a store selector inside the panel; use `useEffect` on its value to call `queryClient.invalidateQueries`.
- Do not trigger a diff fetch from the transcript's `git` system message itself; the panel owns the refresh.
- The `patch` can be up to 1 MiB: parse lazily (memoised on the patch string) and render files with `CollapsibleLines` semantics; do not virtualise in v1.

## Edge cases
- A `sync` git event with `ok: false` still triggers a refetch (the panel then shows the error or the old diff).
- Session `head` that has no commits beyond base: `files` empty, `patch` empty.
- Binary files appear in `files` with `-`/`-` counts from numstat; render counts as `bin`.
- Rename status `R` includes old and new path in the patch header; show the new path.
- Changing `base` to an upstream ref is allowed (read-only).

## Testing
- Vitest: `ChangesFileList` render test with a fixture `Diff` (totals, binary counts, truncated notice) and a test that a `gitEventSeq` change calls `invalidateQueries` (mock `QueryClient`).
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Git operations epic (backend): the diff endpoint including the no-event internal fetch-back.