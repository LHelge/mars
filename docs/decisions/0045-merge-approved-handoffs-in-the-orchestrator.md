# 0045. Approved hand-offs are merged by the orchestrator, configured on the state, and a conflict sends the task back

Status: accepted, written before the implementation (Bears epic `tykeu`). Supersedes ADR 0038 in one respect: `merger` is no longer seeded. Everything else of ADR 0038 stands.

## Context

ADR 0038 seeds four role profiles, and the fourth, `merger`, is an agent whose whole job is one tool call: take the approved hand-off of a task in `merge`, call `merge` with its id, move the task to `done`, or on a conflict move it back to `ready` with the paths. Its prompt says "You do not change code". Every decision in that sequence is already made by the time the task arrives: the tracker knows the current hand-off, whether it is approved, and which branch is the default; the git service knows whether the merge conflicts. Starting a container and a model to make it costs money and a launch, adds a way to fail — a stalled session, a credential that does not resolve, a model that improvises — and, once the dispatcher starts implementers and reviewers unattended, is the last step of the pipeline still waiting for someone to press a button or for an `auto_launch` profile to be configured for a role that needs no judgement.

The question was also what "needs rebasing" should mean for a task that arrives behind the default branch.

## Decision

### The merge is a job, not an agent

An orchestrator job, `auto-merge`, merges approved hand-offs. It runs on a one-minute timer and is woken by the same signals as the dispatcher ("Dispatcher", `ARCHITECTURE.md`), so an approval is normally merged within a second. It acts as `GitActor::System` with the bot identity and a `Requested-By: system` trailer, and as the tracker actor `System`.

Rejected: **keeping the `merger` agent** (ADR 0038's fourth role), for the reasons above. The MCP `merge` tool and the `merger` template remain, for a project that wants an agent in that seat; the template is simply no longer seeded.

Rejected: **merging inside the reviewer's approving `update`**. It would make an MCP call wait on a merge, tie auto-merge to one way of approving, and leave a task a person moves into the state by hand with nobody to merge it. A job over the state catches every way a task gets there.

### Configuration lives on the state

A `queue` state carries `auto_merge` and, when it is set, a `conflict_state`: another `queue` state of the project where a conflicted task goes. The target is always the project's default branch, and success moves the task to the project's first terminal state, the rule parent closure already uses.

Rejected: **a project setting naming the merge state**. The state is what the configuration is about, the states editor is where a person looks at it, and a state carrying its own flag survives a rename without a second reference to repair. Rejected: **a per-task or per-state target branch**. The `merger` prompt's "unless the task says otherwise" cannot be honoured by code that does not read prose, and one integration branch per project is what the push flow and the session base already assume.

### Conflicts only; the merge commit is allowed

The job merges whenever git can, with a merge commit, and sends a task back only when the merge conflicts. The conflicted task goes to the conflict state with a system comment listing the paths; the next implementer starts from the approved hand-off, brings it up to date in its own clone and hands off a new revision, which needs a new review because an approval belongs to the commit that was reviewed (ADR 0018). Nothing rewrites the reviewed commit.

Rejected: **fast-forward only**, landing a hand-off only if it sits directly on the current default branch. It has the strongest guarantee — exactly what was reviewed is what lands — and costs one implementer run and one reviewer run for every other approved task each time anything merges, which with parallel work is most of the pipeline's spending. The combined tree a merge commit produces is untested by the pipeline; the push is where the upstream CI tests it, and a semantic breakage becomes a new task.

### What the job leaves alone, and what it escalates

A held task is skipped: something is working on it. A blocked task is skipped until it unblocks. A task in an auto-merge state without an approved current hand-off is escalated to the human state with the reason: it can only have got there by hand, and leaving it would stall it silently. A paused project (`automation_paused`) is skipped, so the pause keeps meaning "nothing happens in this project by itself".

### Existing projects keep their states

New projects are seeded with `merge` carrying `auto_merge` and `ready` as its conflict state. Existing projects are not migrated: their `merge` state has `auto_merge` off and their `merger` profile stays, as ADR 0038's rule that an upgrade never changes an existing project's behaviour requires. A person turns the flag on in the states editor.

## Consequences

- A new project runs implement → review → merge with two agent roles and no merge profile, and a board with auto-launched implementers and reviewers needs no person until a push.
- The job holds the project git lock through its tracker commit, as hand-off publication does (`ARCHITECTURE.md`, "Git model", "Serialization"); a plain state move by a person does not take the git lock, so a move during a merge can find the code merged and the task elsewhere. The job then comments instead of moving the task. Making every state move take the git lock was rejected as heavier than the problem.
- A conflict send-back is a state change, so it resets `attempts` exactly as a review rejection does. The bound on that loop is ADR 0046's round limit.
- `task_states` gains two columns and the state API two fields; `SPEC.md`, "Role profile templates" marks `merger` as not seeded.
