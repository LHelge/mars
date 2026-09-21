# 0042. Unattended launches are bounded by three caps, paused per project, and recorded as their own launch source

Status: accepted and implemented by the dispatcher (Bears `qabvt`); it is also the contract the scheduled agents (`tup8z`) will be built to.

## Context

In v1 nothing launches a session by itself: every session comes from a person pressing a button, and the ceiling on how many run at once is how fast people press it. The two v2 automation features remove that ceiling. The dispatcher launches an ephemeral session when a state a profile serves has claimable work; the scheduler launches one per due cron tick. Both are *unattended launches*: a launch with no user behind it.

An unattended launch is cheap to start and expensive to be wrong about. A session is a container, a checkout and an agent spending money, and a loop that launches one per task event would take out the host before anybody looked at the board. The rules below were settled with the user while planning the two epics (2026-09-21) and are written down before any of the code, because they are shared: the capacity check, the pause, the eligibility rule and the attribution belong to both features, and only the task selection is the dispatcher's own.

The model they are added to is already in place: profiles declare the states they serve, `ready` returns claimable tasks in those states, the idle reaper fails a stalled ephemeral session, the stuck-task reaper releases its lease, and the attempt limit escalates a task that keeps coming back to the project's human state. What is missing is only who presses the button.

## Decision

### Capacity: three caps, all live sessions, automation only

An unattended launch happens only while the project's live sessions — `creating` or `running` — are below **every** one of:

- `agent_profiles.max_concurrent` for the launching profile,
- `projects.max_concurrent_sessions` for the project,
- the instance-wide `AUTOMATION_MAX_SESSIONS`.

Each cap counts *all* live sessions in its scope, whoever launched them; a session a person started counts against the profile, the project and the instance exactly as a dispatched one does. The caps bind automation only: a launch by hand is never refused by them.

Rejected: **counting only dispatcher-launched sessions**, which makes the cap a promise about the wrong number — a project with ten hand-started sessions would still be handed its full automated allowance, and the figure the user actually cares about is the machine's total load. Rejected: **an instance cap alone**, which is simpler but cannot express "this project may use at most three of the twelve", and would let one busy project's board starve every other project of automation.

### Pause: a project-level switch, no instance switch

`projects.automation_paused` is a toggle on the project page. While it is set, no unattended launch of any kind happens in that project. It takes effect immediately; nothing is restarted, and no configuration is edited.

Rejected: **an instance-wide `DISPATCHER_ENABLED` variable**, because the thing an operator wants to stop is a project whose agents are looping, not every project at once, and because a variable needs a restart to change and cannot be reached from the UI. Rejected: **the profile's `auto_launch` flag alone as the off switch** — turning off each profile in turn is several edits, each of them a destructive edit of a setting the user will have to remember to restore, where a pause is one reversible toggle with one meaning.

### Eligibility: ephemeral, with a resolvable credential, checked at save and again at launch

`auto_launch` and a schedule may be set only on `ephemeral` profiles. A conversational profile exists to talk to a person, and starting one unattended produces a session waiting for input nobody asked for.

Both are refused at save unless the backend's agent credential for that profile resolves without a user, that is at `global` or `project` scope (ADR 0036). An unattended launch has `created_by = NULL`, so a `user`-scope credential can never resolve for one; a profile configured that way would fail every launch it ever attempted. The save-time check turns that into an error the person configuring the profile sees, at the moment they can fix it.

A credential can be deleted after the profile is saved, so the check is repeated at launch: the dispatcher and the scheduler skip such a profile, log it at `info`, and claim nothing.

Rejected: **launch and let it fail**. It is the same information arriving worse: a failed session per task or per tick, each one consuming an attempt against the task and marching it towards the human state for a reason that has nothing to do with the task.

### Back-off: the attempt limit, and nothing else

A launch that fails in `creating` releases the task exactly as a v1 user launch does, and the failure counts as an attempt. The attempt limit with escalation to the project's human state is the only back-off there is.

Rejected: **a per-profile circuit breaker** that stops dispatching for a profile after N consecutive failures. It is a second, invisible mechanism with its own state, its own reset rule and its own way of being stuck off, layered over one that already works and that a person can read off the task: three failures put the task in `needs_human` with three comments saying why. Rejected: **not counting `creating` failures as attempts**, on the argument that the agent never ran. That is exactly the case that must be capped, because a task whose launch fails deterministically — a bad image, a missing credential, a broken repository — would otherwise be retried for ever, and the escalation to a human is the correct end of that loop.

### Ephemeral only, beyond eligibility

An unattended launch produces an ephemeral session: it runs `claude -p`, ends at `result`, fetches its work back and is never parked, resumed or retried. Nothing in either feature launches a conversational session, and nothing revives a finished one; follow-up work is another task and therefore another launch.

### Dispatcher: served states, `ready` order, older profile wins

Three rules are the dispatcher's alone, because the scheduler has no task.

- **Served states are honoured**, unlike a user launch, which ignores them because the person chose the pairing. The dispatcher considers only the `queue` states the profile serves, so a task in the human state, a terminal one, a held one and a blocked one are all out of reach. The escalation to the human state would be worthless if the dispatcher pulled the task straight back out of it.
- **Task order is exactly `tracker::leases::ready_summaries`** — priority, then task number — so what the dispatcher picks is the first row the MCP `ready` tool would have offered the same profile. One ordering, one predicate, and no way for the two to disagree about which task is next.
- **The older profile wins** when two `auto_launch` profiles serve the same state: `agent_profiles.created_at` breaks the tie, deterministically and without a new setting.

### Attribution: `sessions.launch_source`

`sessions.launch_source` records who launched a session: `user`, `dispatcher` or `schedule`. For an unattended launch `created_by` is `NULL` and the tracker actor is `System`.

Rejected: **inferring automation from `created_by IS NULL`**. Deleting a user nulls the column on every session that user launched, so the same `NULL` means both "the machine started this" and "a person who no longer has an account started this". The UI, the logs and any later accounting need the difference, and an explicit column is the only thing that keeps it across a deletion.

## Consequences

- The capacity check is one function over a project, a profile and the instance config, shared by both jobs, and it is the seam `tup8z` is written against. It counts sessions, not launches, so it needs no new bookkeeping.
- Two columns land on `projects` (`max_concurrent_sessions`, `automation_paused`) and one on `sessions` (`launch_source`), so automatic launching is not, as `docs/data-model.md` said, columns on `agent_profiles` and one background job alone.
- `AUTOMATION_MAX_SESSIONS` becomes part of the variable contract in `README.md`, "Configuration", and `.env.example`, with the task that adds it.
- A profile whose credential disappears goes quiet rather than failing loudly. The `info` line naming the profile is the only trace, which is deliberate: the noisy alternative was rejected above.
- Pausing a project does not stop its running sessions or a person's launches; it stops new unattended ones only. Stopping work in flight is what ending a session is for.
- The rules are stated once in `ARCHITECTURE.md`, "Task tracker" → "Unattended launches"; the "Dispatcher" section and the later "Scheduled agents" section refer to them rather than restating them.
