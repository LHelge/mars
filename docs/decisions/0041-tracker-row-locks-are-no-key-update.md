# 0041. Tracker row locks are `FOR NO KEY UPDATE`, so they never block a foreign-key check

Status: accepted. Refines the lock strength of ADR 0021.

## Context

A `PUT /projects/{pid}/tasks/{n}` with a hand-off answered 500 now and then, and the Postgres log showed a deadlock (`40P01`) between that tracker mutation and the session owner committing a transcript line (Bears `kb48s`, `kz9mx`). ADR 0021's order — project row before session row — was being followed by both; the second lock was one neither wrote.

The owner's transaction locks its session row and nothing else. But when one transaction updates the same row twice, Postgres re-runs that row's foreign-key checks on the second update, because the version being replaced is the transaction's own and its checks have not been committed. A `result` line updates `sessions` twice (`last_seq`, then the cost counters); a state transition with its event does too. The re-check takes `FOR KEY SHARE` on every row `sessions` references: the project, the profile, the user, the task, the hand-off. The mutation held the project and the task `FOR UPDATE`, which conflicts with `FOR KEY SHARE`, and was itself waiting for `FOR KEY SHARE` on the session row for a foreign key of its own. Two statements of plain SQL reproduce it.

Options considered:

1. Keep `FOR UPDATE` and retry the mutation on `40P01`. Rejected as the fix: a mutation is not a closure that can be re-run — it is opened, used by its caller across several awaits and committed — a hand-off has pinned a git ref by then, and the cycle would still cost a `deadlock_timeout` every time it formed.
2. Make every session-only transaction write its `sessions` row exactly once. Rejected: a dozen call sites compose a transition, an event batch, a container id and counters from separate repository methods, the rule would be invisible at each of them, and the next second `UPDATE` would bring the deadlock back without a test noticing.
3. Have session-only writers take `FOR KEY SHARE` on the project first, making the order explicit. Rejected: every transcript line would queue behind every mutation of its project, and share locks jump the queue past a waiting exclusive lock, so a few streaming sessions could starve the tracker.
4. Lock tracker rows `FOR NO KEY UPDATE`. Chosen.

## Decision

- `ProjectRepository::lock_project` and `TaskRepository::find_task_for_update` lock `FOR NO KEY UPDATE`. It conflicts with itself and with `FOR UPDATE`, so ADR 0021's serialisation is unchanged, and it does not conflict with `FOR KEY SHARE`, so no foreign-key check waits for a mutation. It is the strength an `UPDATE` that leaves the key alone takes anyway, and no mutation changes a project's or a task's key.
- A caller that holds a row exclusively by nature orders its locks instead. `TaskRepository::delete_task` locks the sessions naming the task or one of its hand-offs, in id order, before the task row.
- Project deletion and the shared-directory clear and delete keep `FOR UPDATE`, as `lock_project_exclusive`: a launch without a task inserts its session outside any project lock, and that insert's foreign-key check waiting for them is what makes their live-session count authoritative. They refuse while a session is live, so they never wait for a session row that has a writer.

## Consequences

- The session owner's hidden foreign-key locks stay hidden and stay harmless; nothing in `session/` changed.
- Inserting a row that references a project — a session, a profile — no longer waits for that project's tracker mutation.
- User deletion is a third caller that orders its locks (Bears `u9fvv`): the user row `FOR NO KEY UPDATE`, the tracker lock of every project with a row naming the user, the user's sessions, and the `DELETE` last. Profile deletion needs no ordering, because it refuses a profile with any session and a racing launch waits for nothing it holds.
- The rows of `secrets`, `user_invites` and the two token tables that name a user are cleared or removed by the same `DELETE` without being locked first. Their writers hold one row briefly and none has been seen to meet a deletion; the ordering above is the pattern if one does.
- `tests/tracker_mutation.rs` and `tests/users_delete_locks.rs` hold the interleavings deterministically; each failed with `40P01` before the change. `tests/repositories_profiles.rs` holds the launch racing a profile deletion.
