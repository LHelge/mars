---
id: gm6te
title: "Component fixes: push-rejected banner, password form clearing the wrong fields, render bound for huge diffs and JSON"
status: open
priority: P3
created: "2026-09-21T10:55:22.372010812Z"
updated: "2026-09-21T10:55:22.372010812Z"
tags:
  - frontend
  - technical-review
  - bug
parent: "579dz"
---

Problem, each small:
- components/git/PushForm.tsx:161 interpolates the live "Remote branch" input into the rejected banner, so after a 409 editing the field rewrites the advice ("merge origin/<whatever is typed>"). The 409 also rethrows at :83, so the warning Alert and the error Alert with the server text both show for one event. remote_branch is sent untrimmed (:70) while the enable check trims (:138).
- components/PasswordChangeForm.tsx:90-96 wipes password and confirm and keeps currentPassword on any failure. For the 400 "current password is incorrect" that discards the two correct fields and keeps the wrong one — the opposite of the comment's intent — and the server error lands in the top Alert rather than on the field, which already has `error=` plumbing.
- No render bound for large content: collapse is per file (> 500 changed lines) but nothing caps the total, so a 1 MiB patch of 100 files x 200 lines mounts about 20k rows x 5 elements synchronously (components/git/DiffBody.tsx:19/:45-57, DiffView.tsx:124-133); JsonTree opens depth 0 and 1 with no limit on entries, so a tool result that is a 10k-element array mounts 10k stateful Nodes (JsonTree.tsx:80-107). lineDiff is properly capped; rendering is not. (Plausible; not measured.)
- DiffBody.tsx:49-55 passes an inline ref callback per file, so every render deletes and re-sets every entry of the block map; harmless today, and it makes memoising PatchFileBlock pointless later.
- DiffView exposes no add/delete semantics to assistive technology (the marker is aria-hidden, DiffView.tsx:38-40; split mode has colour and side only), and each HalfRow span has its own overflow-x-auto, so long lines get one scrollbar per row rather than one per pane.
- utils/github.ts:61 interpolates ref names unencoded, so a branch like `fix#12` or one containing ? or % produces a compare link whose tail becomes a fragment or query.
- Dead props that imply behaviour which does not exist: MergeForm `mode?: "branch"` (one member, no caller passes it), PasswordChangeForm `requireCurrent` (no caller), StatusBadge's human/cloning/ready/error members and `label` (both call sites pass a SessionState; project status is drawn by pages/projects/ProjectStatusPill, which colours `error` red where StatusBadge colours it muted).

Acceptance: the banner names the branch that was rejected (store it instead of the boolean), a handled 409 returns early as MergeForm does for a 422, and the name is trimmed once. A wrong current password clears only that field and shows the error on it. Total changed lines past a budget collapse all files, and JsonTree caps entries per branch with a "show N more" as StringValue does for characters. Stable per-file element lookup (id/data-path or one container ref). sr-only "added"/"removed" per diff row and overflow on the two pane containers. Each "/"-separated ref segment is encodeURIComponent'd. Delete the dead props, or merge ProjectStatusPill into StatusBadge. Tests for the banner, the password fields and the URL encoding.

References: files above; frontend/src/components/git/MergeForm.tsx (the pattern to follow). Contract: SPEC.md, "Git" (push 409, merge 422) and "Frontend".