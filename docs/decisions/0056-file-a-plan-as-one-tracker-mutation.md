# 0056. A plan is filed as one tracker mutation; not by prompt ordering, a wider dispatch gate, or self-promotion from `backlog`

Status: accepted (Bears epic `utese`, task `egnkb`).

## Context

A planner files a graph: a parent, its sub-tasks and the `blocks` edges between them. With `create_task` alone it does so one call at a time, and every call is its own tracker mutation — its own commit and its own `task_events` notification, each of which wakes the dispatcher (`ARCHITECTURE.md`, "Dispatcher"). Between two calls the tracker holds part of the graph. A dependant filed before its prerequisite, or before the `update` that adds the edge to it, is an unblocked task in a queue state, and a dispatcher run in that window launches an implementer on it. Coalescing wake-ups narrows the window; it cannot close it, because a run that starts between two calls sees exactly what those calls committed.

ADR 0052 does not cover this either. It holds a task back until its author's *commits* have landed on the default branch; a planner that has committed nothing, or whose work has already landed, passes that check with its graph half filed.

## Decision

A new MCP tool, `create_plan`, files a parent (new, or an existing top-level task such as the one a planner was launched for), up to 50 sub-tasks and the `blocks` edges between them as one `TrackerMutation`: one project lock, one transaction, one batch of `TaskEvent` rows and one notification (`SPEC.md`, "MCP tool contracts" → `create_plan`). Sub-tasks name each other by caller-chosen `ref`s, which can never be read as task references, and may also depend on existing tasks. Each task goes through `create_task`'s own creation path, so state, parent, edge, `blocked`, provenance and event rules are the same ones; the batch adds local refs, a creation order that puts every prerequisite first, a cycle check over the local edges before the lock, and one provenance for the whole plan. Any failure creates nothing. The dispatcher's candidate read, like every other reader, sees either none of the plan or all of it, with each dependant already `blocked`.

The planner template is switched to the tool separately (task `na7y4`); `create_task` stays for single discovered tasks.

Rejected:

- **Prompt ordering alone** — telling the planner to file prerequisites first and to give every task its `depends_on` at creation, never to add a blocking edge to a ready task afterwards. It is worth saying and the planner template says it, but it relies on the model following an instruction every time, and a single slip is a launched implementer on work that is not ready. The guarantee belongs in the tracker.
- **Widening ADR 0052's gate** to hold a task back while its author session is still live. Planners are conversational and park rather than end, often for days, so every planner-filed task would wait for somebody to end a session that has no reason to end; and the gate would still let through the tasks of a planner that did end with its graph half filed.
- **Letting the creator promote its own tasks from `backlog`** — file everything in `backlog`, then move the tasks to `ready` once the graph is complete. Every promotion is again its own mutation, so the same race reappears whenever the promotion order is wrong: a dependant promoted before its prerequisite's edge exists, or a prerequisite still in `backlog` while its dependant is `ready`, and it adds a lease exception for the creator to a state change that is otherwise the holder's.

## Consequences

- A planner's graph appears on the board, and to the dispatcher, all at once, and a failed plan leaves no trace: no tasks, edges, events or task numbers.
- One call holds the project lock for up to 51 task creations. The bound of 50 sub-tasks keeps that short; a larger plan is split into several parents.
- A plan cannot add an edge from an existing task to a new one. Nothing that exists can depend on a task of the plan, which is also why edges to existing tasks need no batch-wide cycle check.
