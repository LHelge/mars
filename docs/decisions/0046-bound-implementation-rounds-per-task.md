# 0046. Implementation rounds are counted per task and bound send-backs; `attempts` keeps resetting

Status: accepted, written before the implementation (Bears epic `tykeu`).

## Context

`attempts` counts claims since a task last changed state and escalates a task to the human state when a *release* finds it at `max_attempts` (ADR 0016). That bounds a step that keeps failing. It does not bound a step that keeps succeeding in a circle: an implementer hands off to `review`, a reviewer forwards the hand-off back to `ready` with `changes_requested`, and each move resets `attempts`. `ARCHITECTURE.md` said only the reviewer's prompt and the comment history stop that loop. With the dispatcher launching implementers and reviewers unattended (ADR 0042), and auto-merge adding a second way back to `ready` on a conflict (ADR 0045), the loop spends money with nobody watching.

## Decision

`tasks.rounds` counts revision hand-offs published since the task last left the project's human state. Every trip through an implementer produces exactly one revision, whatever the project calls its states, so the count needs no state names.

A **send-back** is a forwarded hand-off that records `changes_requested`, or an auto-merge conflict. When a session or the system sends back a task whose `rounds` has reached `projects.max_rounds` (default 5), the move goes to the human state instead of the requested one, with an `escalated` event, the reason and the escalation email. A person's own send-back from the UI is not redirected: the limit exists to stop automation, and a person who asks for changes has decided.

Leaving the human state resets `rounds` to 0, so a person who hands an escalated task back with a decision gives it a fresh allowance.

Rejected: **no longer resetting `attempts` on a state change**. Escalation is checked on release, and a hand-off loop never releases, so a growing `attempts` would pass `max_attempts` unnoticed; and `attempts` would turn from "failures at this step" into "claims over the task's life", so a healthy task going through planner, implementer, reviewer and one rejection would reach the default limit of 3 having failed at nothing, and the cap on deterministic launch failures that ADR 0042 relies on would have to be loosened.

Rejected: **checking the limit where the dispatcher launches**. The dispatcher does not know which launch starts a new round — a reviewer launched on the fifth revision must still run — and a check there would bound unattended launches only, not a person-launched agent in the same loop.

## Consequences

- One column on `tasks`, one on `projects`, a `rounds` field on `Task` and `max_rounds` on `Project`.
- The redirect is decided inside the tracker mutation that would have sent the task back, under the project lock, so it composes with the hand-off forward and with the auto-merge job's move exactly as the release escalation composes with a release.
- A reviewer's `update` to `ready` can come back with the task in the human state; the MCP output shows the state the task actually went to.
