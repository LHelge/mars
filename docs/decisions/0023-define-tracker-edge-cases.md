# 0023. Define tracker edge cases without adding workflow machinery

Status: accepted. Clarifies the tracker contract from ADR 0016.

## Context

The tracker design leaves several boundary cases ambiguous: assigning an unchanged state, removing the final queue, deleting prerequisites, nesting parents, choosing an origin when a session holds several tasks, and recording multiple relationship kinds between the same tasks. REST, MCP and background jobs need the same answers before implementation.

Options considered:

1. Treat every state assignment as a hand-off. Rejected: saving an unchanged state could release another worker's lease or erase its attempt count.
2. Infer discovery provenance from an arbitrary held task. Rejected: a session may legitimately hold several tasks, and the orchestrator cannot know which one led to the discovery.
3. Allow only one relationship kind per pair. Rejected: where work came from and what must finish first are independent facts.

## Decision

- Assigning the current state through an update preserves the lease, attempts and closure timestamp and emits no state-change event. Other fields may still change. Explicit release actions remain available; code hand-offs still require a different target state.
- A project retains at least one queue state, exactly one human state and at least one terminal state. Reject deletion of the final queue, even when empty.
- Deleting a prerequisite recomputes surviving dependants' blocked flags after removing its edges, within the same project-locked transaction. Capture affected identities before the cascade, account for other blockers and children, and publish dependency-removal and flag-change events with the deletion.
- Parent nesting is one level, including terminal tasks. A task with children cannot acquire a parent; a task with a parent cannot receive children. Validate both ends on creation and re-parenting, including same-project and non-self checks.
- MCP task creation accepts an optional `discovered_from` origin. Omission infers the sole held task, records no origin when none is held, and fails when several are held. An explicit origin must be held by the caller in the project. If it is also the new task's parent, the parent link suffices; otherwise create the provenance edge.
- Dependency identity includes both task IDs and the relationship kind. A `blocks` edge may coexist with a `discovered_from` or `related` edge for the same pair. Removal specifies a kind; existing MCP dependency-edit fields affect only `blocks`.

All validation and derived changes use the existing per-project transaction discipline (ADR 0021).

## Consequences

- These rules add validation and precise mutation semantics without adding a scheduler or another workflow state.
- Clients can save a task without accidentally handing it off. Agents holding multiple tasks must identify discovery provenance explicitly.
- Acceptance covers unchanged-state updates on held and closed tasks, final-queue deletion, prerequisite deletion with and without other blockers, both directions of invalid nesting, ambiguous discovery origins, and independent insertion/removal of relationship kinds for one pair.
