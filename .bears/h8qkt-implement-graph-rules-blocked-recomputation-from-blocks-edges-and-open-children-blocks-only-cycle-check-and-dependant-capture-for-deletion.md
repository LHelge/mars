---
id: h8qkt
title: "Implement graph rules: blocked recomputation from blocks edges and open children, blocks-only cycle check, and dependant capture for deletion"
status: in_progress
priority: P1
created: "2026-09-16T20:41:10.506405006Z"
updated: "2026-09-19T10:10:25.664900980Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - thes7
  - tepsh
parent: "5h3y4"
attempts: 1
---

## Summary
Implement the stored-`blocked` maintenance and the dependency-graph validation every later mutation relies on: recompute `blocked` for a set of tasks from their non-terminal `blocks` prerequisites and non-terminal children, emitting `blocked`/`unblocked` on each flip; reject a `blocks` edge that would form a cycle using a recursive CTE run under the project lock; and capture a task's surviving dependants and parent before a deletion cascades its edges. All functions take `&mut TrackerMutation` and never open their own transaction.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "Blocked is stored" (recompute for all dependants when a `blocks` edge is added or removed, a task enters or leaves a terminal state, or a prerequisite is deleted; each flip emits `blocked`/`unblocked`; on deletion capture dependants before cascading, recompute from surviving prerequisites and children, emit `dependency_removed`), "Parents" (a non-terminal child blocks its parent the same way; recompute when a child is created, deleted, re-parented or changes state).
- `docs/data-model.md` `tasks.blocked` ("True while any `blocks` dependency or any child is in a non-terminal state"), `task_dependencies` (cycles among `blocks` edges rejected with a recursive CTE after locking the project row and before insert; other kinds never checked and never affect `blocked`; `task_dependencies_depends_on_idx`).
- `SPEC.md` "Tasks" (409 on cycle; only `blocks` edges participate in cycle checks; `blocked`/`unblocked` event kinds).
- ADR 0021 (reciprocal edges serialise; the second fails validation), ADR 0023.

## Acceptance criteria
- [ ] `orchestrator/src/tracker/graph.rs` exposes `pub async fn recompute_blocked(m: &mut TrackerMutation, task_ids: &[Uuid]) -> Result<Vec<BlockedFlip>>` where `BlockedFlip { task_id, blocked: bool }`; for each task it computes `EXISTS (blocks prerequisite whose state kind <> 'terminal') OR EXISTS (child whose state kind <> 'terminal')`, updates `tasks.blocked` and `updated_at` only when the value changes, and emits `blocked` or `unblocked` (payload: actor + full task) per flip, in the order of `task_ids`.
- [ ] `pub async fn dependants_of(m, task_id) -> Result<Vec<Uuid>>` returns tasks with a `blocks` edge on `task_id`; `pub async fn affected_by_state_change(m, task_id) -> Result<Vec<Uuid>>` returns those dependants plus the task's parent (if any), deduplicated: the set to recompute after a terminal transition either way.
- [ ] `pub async fn check_no_cycle(m, task_id, depends_on) -> Result<()>` runs, under the already-held project lock, a recursive CTE over `task_dependencies WHERE kind = 'blocks'` starting at `depends_on` and following `depends_on_task_id`; if it reaches `task_id` (or `task_id == depends_on`) it returns `Error::Conflict("dependency would create a cycle")`. Only `blocks` edges are traversed; `discovered_from` and `related` edges are ignored both as start and as path.
- [ ] `pub struct DeletionCapture { dependants: Vec<(Uuid, TaskDependencyKind)>, parent_id: Option<Uuid>, children: Vec<Uuid> }` and `pub async fn capture_before_delete(m, task_id) -> Result<DeletionCapture>` lists every incoming edge (all kinds, with kind), the parent and the children, so the delete task can emit `dependency_removed` per surviving dependant per removed kind and recompute `dependants ∪ {parent}` after the cascade (children lose their `parent_id` via `SET NULL` and are not blocked by the deleted task, so they need no recompute).
- [ ] `blocked` is never recomputed for a task from the human or terminal kind differently: the flag is purely a function of prerequisites and children; a terminal task can be `blocked = true` if it still has open children (the flag is only consulted for claimability).
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/tracker/graph.rs`; SQL may live in `orchestrator/src/repositories/tasks/` as helpers taking `&mut PgConnection` (e.g. `compute_blocked(conn, task_id) -> bool`, `set_blocked(conn, project_id, task_id, bool) -> bool changed`, `blocks_reaches(conn, from, target) -> bool`).
- Cycle CTE shape: `WITH RECURSIVE reach(id) AS (SELECT $2::uuid UNION SELECT d.depends_on_task_id FROM task_dependencies d JOIN reach r ON d.task_id = r.id WHERE d.kind = 'blocks') SELECT EXISTS (SELECT 1 FROM reach WHERE id = $1)`; the scope check (both tasks in `project_id`) is the repository's `insert_dependency` job, but `check_no_cycle` must run before that insert in the same mutation.
- The blocked computation joins `task_states` on `state_id` to read `kind`; keep it as one SQL statement per task or one statement for the whole set (`= ANY($1)`), not per-edge Rust loops.
- Emit through `m.emit_task(TaskEventKind::Blocked | Unblocked, &dto)` after the update so the payload shows the new flag.

## Edge cases
- A task that both is a dependant and the parent of the changed task is recomputed once.
- Recomputing an empty set is a no-op.
- Self-edge `task == depends_on` is rejected by the CTE path as a cycle before the `CHECK` constraint fires.
- A `blocks` edge from a task to a terminal task is legal and leaves `blocked` false.
- Concurrent reciprocal insertions: because both mutations hold the project lock, the second one's CTE sees the first edge and fails; add a test that runs two mutations from two tokio tasks and asserts exactly one succeeds.

## Testing
- Integration tests in `orchestrator/tests/tracker_graph.rs`: chain A→B→C of `blocks` edges, adding C→A returns `Conflict("dependency would create a cycle")` while `related` C→A is accepted; `recompute_blocked` flips a dependant to blocked when its prerequisite is open and emits exactly one `blocked` event; recompute again emits nothing; a parent with one open child is blocked, with all children terminal is not; `capture_before_delete` returns each incoming kind separately for a pair with `blocks` and `discovered_from`; the reciprocal concurrent-edge test above.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `task_dependencies` index on `depends_on_task_id`, `TaskRepository::list_dependants`, `list_children`, `insert_dependency` (same-project check).