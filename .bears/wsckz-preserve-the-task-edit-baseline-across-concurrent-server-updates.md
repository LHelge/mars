---
id: wsckz
title: Preserve the task edit baseline across concurrent server updates
status: open
priority: P1
created: "2026-09-21T10:44:25.262432502Z"
updated: "2026-09-21T10:49:31.120348428Z"
tags:
  - frontend
  - technical-review
  - bug
  - tracker
parent: "579dz"
---

Problem: frontend/src/tasks/TaskEditForm.tsx recomputes original with useMemo([task]), while draft fields initialized with useState retain the values from edit start. An SSE/query refresh changing priority from P2 to P0 makes a title-only save submit priority P2 as well. A temporary component regression test confirmed the incorrect payload.

Acceptance: capture baseline and draft together for the lifetime of an edit, or track explicitly edited fields. Untouched fields must never be submitted merely because server data changed. Preserve partial-update semantics, explicit null clearing, validation, and no-op saves. Add component coverage that rerenders the same task with a concurrent priority/description change and verifies a title-only save leaves those fields out.

References: frontend/src/tasks/TaskEditForm.tsx:63 and :131; frontend/src/tasks/taskEdit.ts. Contract: SPEC.md, "Tasks" (PUT semantics) and "Frontend", "Board refresh ordering".

Merged from the second review (2026-09-21), same finding, additional detail:
- The file header (TaskEditForm.tsx:4-7) and the doc comment on diffTaskInput (taskEdit.ts) both promise that the diff prevents this overwrite; diffTaskInput itself is correct, it is fed a moving baseline. The smallest fix is `const [original] = useState(() => taskEditValues(task))`.
- Optional: when taskEditValues(task) diverges from the snapshot while the form is open, say so ("this task changed while you were editing") rather than staying silent.
- TaskEditForm takes `task: Task` but always receives a TaskDetail and re-derives hasChildren from the board snapshot (taskEdit.ts hasChildren); `task.children.length > 0` is authoritative and already in the prop.
- The same "state captured at mount, prop keeps moving" shape in the review forms and MoveToState is tracked separately in the hand-off pinning task of this epic.