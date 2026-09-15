# Open questions

Everything the brief left undecided that has not yet been resolved. Each entry states the options, a recommendation, and whether the other documents already assume the recommendation. Resolve an entry by moving its outcome into the relevant document (and an ADR if an alternative was rejected) and deleting it here.

The first review pass (2026-09-15) resolved 30 questions; their outcomes are in `SPEC.md`, `ARCHITECTURE.md`, `docs/data-model.md`, `README.md` and ADRs 0013 and 0014. One remains.

## Tasks

### 1. How task assignment is modelled

Deferred to a dedicated task-structure planning session.

The brief says the MCP `update` tool sets an assignee. Three readings are all useful: a role (which profile should pick the task up), a session (which agent owns it), and a user (which human should look at it). The documents currently carry all three as separate columns (`tasks.role`, `tasks.assignee_session_id`, `tasks.assignee_user_id`), with `claim` setting the session and `needs_human` typically preceding a human assignment. Alternatives are dropping the human assignee, or a single polymorphic assignee column with a kind discriminator.

The planning session should also settle what else belongs on a task (labels, estimates, a parent/epic relation, ordering within a state), since those decisions interact with assignment and with the future policy engine that spawns agents when tasks become ready.

Recommendation: keep the three provisional columns until that session; nothing outside the task tables depends on their shape, and `SPEC.md` and `docs/data-model.md` mark them provisional.
