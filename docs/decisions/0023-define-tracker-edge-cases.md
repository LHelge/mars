# 0023. Define tracker edge cases without adding workflow machinery

Status: accepted. Clarifies the tracker contract from ADR 0016.

## Context

The tracker design left boundary cases ambiguous: assigning an unchanged state, removing the final queue, deleting prerequisites, nesting parents, choosing an origin when a session holds several tasks, and recording several relationship kinds between the same tasks.

Options considered:

1. Treat every state assignment as a hand-off. Rejected: saving an unchanged state could release another worker's lease or erase its attempts.
2. Infer discovery provenance from an arbitrary held task. Rejected: a session may hold several tasks and the orchestrator cannot know which led to the discovery.
3. Allow one relationship kind per pair. Rejected: where work came from and what must finish first are independent facts.

## Decision

- Assigning the current state through an update preserves lease, attempts and closure timestamp and emits no state-change event. Code hand-offs require a different target state.
- A project keeps at least one queue state, exactly one human state and at least one terminal state.
- Deleting a prerequisite recomputes surviving dependants' blocked flags in the same project-locked transaction.
- Parent nesting is one level, terminal tasks included.
- MCP task creation accepts an optional `discovered_from`. Omission infers the sole held task, records nothing when none is held, and fails when several are held.
- Dependency identity is both task ids plus the kind, so `blocks` may coexist with `discovered_from` or `related` for one pair.

All validation follows the per-project transaction discipline (ADR 0021).

## Consequences

- Precise validation and mutation semantics without a scheduler or another workflow state.
- Clients can save a task without handing it off. Agents holding several tasks name discovery provenance explicitly.
