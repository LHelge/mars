---
id: unh3n
title: Coalesce task-stream invalidations, stop replaying the whole event history on first connect, and invalidate details on states_changed
status: done
priority: P2
created: "2026-09-21T10:52:54.939856108Z"
updated: "2026-09-21T19:25:51.527498418Z"
tags:
  - frontend
  - technical-review
  - bug
  - tracker
  - performance
parent: "579dz"
attempts: 1
---

Problem, in the task stream and board refresh path (the ordering logic itself was traced through event-during-flight, project switch, reconnect, unmount and reset and holds; these are the exceptions):
- First connection replays everything: bindProject sets lastSeq 0 (tasks/taskStore.ts:126), services/tasks.ts builds `after=0`, the server pages through task_events from the beginning and nothing prunes that table. Per replayed event the client does a JSON.parse of a full Task payload, a Zustand set that runs every card's selector, one prefix invalidateQueries and, for 4 of the 13 kinds, a predicate scan of the whole query cache (tasks/useTaskStream.ts:137-160). Each event bumps eventGeneration, so the refresh started at open is discarded and restarted for as long as the replay outlasts a round trip: first paint of the board waits for the replay. On a deep link the detail query is active, and every replayed event refetches it with cancelRefetch; apiClient threads no AbortSignal, so N historical events are roughly N real GET /tasks/{n}. Cost grows with project age.
- states_changed never invalidates open task details: the early return on `task_id === null` (useTaskStream.ts:147-148) precedes the taskKeys.all invalidation, and a rename emits only states_changed with task_id null. The drawer keeps the old state name (MoveToState.tsx:71-75 has a fallback option written for the symptom), and launchRules.defaultProfile matches no serves_states and silently falls back to the first profile, until a focus refetch. SPEC.md, "Board refresh ordering": open task details are invalidated on task events as well.
- The session-tasks invalidation (useTaskStream.ts:50-60, :154-160) never meets an active observer: TaskStream exists only while BoardTab is mounted and TasksPanel lives on the session page. It only marks inactive entries stale, which staleTime already does, at the price of a full cache scan per event; its predicate also re-spells the shape of queryKeys.sessions.tasks structurally, so a key change would break it silently.
- fetchQuery de-duplication (taskStore.ts:246-259): a refresh can join a read that began before it. R1 starts both reads, states fails fast, Promise.all rejects and loading clears while the tasks GET is still in flight; an event starts R2, whose fetchQuery(tasks) joins R1's pre-event GET and installs it with no generation mismatch. Plausible, low frequency; the change stays missing until the next event.
- Every local mutation costs two full refreshes (invalidate() and the mutation's own SSE event each bump the generation: 4 GETs per write, often with a wasted first round trip), and under sustained events faster than one round trip the board installs nothing and shows a permanent "Refreshing". That label is `role="status"` (TaskBoard.tsx:125-132) and flips on every event, so a screen reader hears it continuously while agents work.
- fetchQuery resolves with the raw response, not the structurally shared query.state.data, so every Task object is new on every event and memo(TaskCard) only helps sibling renders; selectTaskById is an O(n) find per card and dependency row on every store write, including each search keystroke.

Acceptance: query invalidations from the stream are coalesced (one flush per tick/frame, or deferred until the first snapshot is loaded) and a first connection does not start at seq 0 — let the client learn the current seq from the snapshot or an endpoint, with SPEC.md updated if the contract changes. states_changed invalidates taskKeys.all before the null check. The dead session-tasks predicate is removed or moved to where an observer exists. A refresh never installs a read that began before it (cancelQueries before fetchQuery, or call the services and setQueryData). Decide, with a SPEC.md/ADR 0022 note, whether a dirtied-but-ordered response may be installed before the follow-up run to end the starvation and the double read. "Refreshing" is shown only after a delay or for a non-live stream. Unchanged tasks keep their identity across refreshes (read back getQueryData) and lookups by id use a map built once per snapshot. Tests at the store seam for each ordering fixed.

References: frontend/src/tasks/taskStore.ts, useTaskStream.ts, useTaskMutations.ts, TaskBoard.tsx, TaskCard.tsx; frontend/src/services/tasks.ts; orchestrator/src/sse/mod.rs. Contract: SPEC.md, "SSE: task stream" and "Frontend", "Board refresh ordering"; ADR 0022.