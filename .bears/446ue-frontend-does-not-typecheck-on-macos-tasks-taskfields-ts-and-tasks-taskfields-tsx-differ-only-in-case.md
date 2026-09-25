---
id: "446ue"
title: "Frontend does not typecheck on macOS: tasks/taskFields.ts and tasks/TaskFields.tsx differ only in case"
status: done
priority: P2
created: "2026-09-25T10:18:17.315662Z"
updated: "2026-09-25T10:18:27.600062Z"
tags:
  - frontend
---

Found while verifying epic `xz6yq` on macOS. On a case-insensitive file system, `import "./TaskFields"` resolves to `taskFields.ts`. `tsc -b` then fails with TS1261 and TS2724, and lint, the build and `TaskEditForm.test.tsx` fail with it. Fixed by renaming the hook module to `src/tasks/useTaskFields.ts`, after the hook it exports.