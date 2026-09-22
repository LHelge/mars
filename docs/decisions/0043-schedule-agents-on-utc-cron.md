# 0043. Scheduled agents fire on a 5-field UTC cron, skip what they miss, and carry their own prompt

Status: accepted, written before the implementation (Bears epic `tup8z`); the unattended-launch rules it builds on are ADR 0042.

## Context

The second half of v2's automatic launching is the scheduled agent: a profile with a cron expression, launched with nobody asking at each tick, with no task behind it. The tech-debt scanner that files `ready` tasks once a day is the plain case; the agent that turns new GitHub issues into `backlog` tasks is the same thing waiting for GitHub access, which is on the roadmap in `README.md`.

Everything about *whether* such a launch may happen is already decided and written down: ADR 0042 settled the three caps, the per-project pause, the ephemeral-only and resolvable-credential eligibility, the `created_by` NULL attribution and `sessions.launch_source`, deliberately as rules shared by the dispatcher and the scheduler rather than as the dispatcher's own. What is left, and what this ADR records, is *when* a launch happens and *what the agent is told*: the expression, the clock it is read in, what happens to a tick nobody was up to see, what happens to a tick the caps refuse, and where the run's instructions come from.

The rules below were settled with the user while planning the two epics (2026-09-21), before any of the code, because the schema task, the job task and the UI task all depend on the same answers. The implementation tasks refine them — the job defines "due" as an occurrence in `(max(last_scheduled_at, process start), now]` and the schema fixes the tick at one minute — and none of them reopens them.

## Decision

### Expression: standard 5-field cron, evaluated in UTC always

A profile's schedule is a standard 5-field cron expression — minute, hour, day of month, month, day of week — the form a person already knows from `crontab`. It is evaluated in **UTC**, always, with no time zone stored anywhere: not on the instance, not on the profile. The UI renders the *next run* of a saved schedule in the viewer's own local time, so the one place a time zone matters — reading off when the thing will actually happen — is served by the browser, which knows the answer without being configured.

Rejected: **an instance time zone**, a variable naming the zone every schedule is read in. It is one more required piece of configuration, it changes the meaning of every existing schedule when an operator edits it, and it is wrong for any instance whose users are not all in one place. Rejected: **a per-profile time zone**, which is strictly more expressive and strictly more to get wrong: it puts a zone database in the schema and in the validation, and it makes "3 am" a question with a different answer per profile, twice a year, for a daily scan whose hour nobody cares about to the minute. A schedule here is machine work, not an appointment; UTC makes every schedule comparable, makes a restart or a host move a non-event, and has no discontinuities to reason about at a daylight-saving boundary.

### The crate: `croner`

`croner` (hexagon/croner-rust) is the crate the scheduler parses and evaluates with. It is named in `ARCHITECTURE.md`, "Orchestrator internals", with the other per-concern crates, and added with `cargo add` by the task that first uses it.

Checked against the four things this decision needs:

- **5-field support.** `croner`'s parser is built with the seconds and year fields each `Optional`, `Required` or `Disallowed`. The implementation (Bears `6f9uu`) builds it with `Seconds::Disallowed` and `Year::Disallowed`, which is *stricter* than the `Seconds::Optional` this ADR first described: with `Optional` the crate would also accept a 6- or 7-field expression and read its first field as seconds, and an expression whose meaning depends on how many fields it has is the trap this decision exists to avoid. The `@daily` family is expanded by the crate before the field count is looked at, so it is refused by a shape check in front of the parser rather than by the parser. The day-of-month/day-of-week pair follows the usual Vixie OR semantics, with an opt-in AND.
- **UTC evaluation and "next occurrence after t".** `Cron::find_next_occurrence(&start_time, inclusive) -> Result<T, CronError>` is generic over the datetime type, and `chrono::DateTime<Utc>` is one of them, which is precisely the question the job asks once a minute and precisely one value rather than an iterator to take from. Verified against 4.0.0 as this signature. It is also what the API's computed `next_scheduled_at` is; an expression that can never match — the 30th of February — exhausts the crate's search limit and is answered as "no next run" rather than as an error.
- **Maintenance.** Actively released (4.0.0, 2026-08-31, after a 3.x line through 2025), MIT, with `chrono` as the feature the orchestrator already depends on and no heavy transitive additions.

Rejected: **`cron` (zslayton/cron)**, the older and more downloaded crate, which is well maintained (0.17.0, 2026-06) but has no 5-field mode: its parser reads seconds, minute, hour, day of month, month, day of week and an optional year, so a standard 5-field expression does not parse at all, and a user pasting a 6-field one would silently get an expression shifted by a field — `0 3 * * * *` meaning "every second of 3 am" rather than "3 am". Prefixing a `0` for the user before parsing was considered and dropped: it turns every error message, every round trip through the UI and every stored value into a question about which form is in hand. Rejected: **`saffron`**, which is Quartz-flavoured and a single 0.1 release from 2020.

The crate parses; it does not schedule. No scheduling framework is adopted: the tick is a `cron/` job like every other one ("Background jobs").

### Missed ticks are skipped, never caught up

Only a tick that comes due while the orchestrator is up fires. Nothing is replayed after a restart, a crash or an outage: an instance that was down from Friday to Monday launches nothing at all on Monday morning beyond Monday's own due ticks.

Rejected: **one catch-up launch at startup** for a schedule whose last occurrence was missed. It sounds harmless and is not: a restart during a deployment would launch a session per scheduled profile in the same second, on top of whatever the dispatcher is doing, for work whose whole point is that it happens periodically. A daily scan that misses Sunday is not worth a session on Monday morning that costs money and files a second set of tasks; the next ordinary tick does the same job. The alternative also needs a rule for how far back to catch up, which is a knob nobody asked for. "It fires while we are up" is a sentence an operator can hold in their head.

### Overlap is bounded by the caps, and a refused tick is spent

A due tick launches unless the unattended-launch capacity check of ADR 0042 says no — the profile's `max_concurrent`, the project's `max_concurrent_sessions`, the instance-wide `AUTOMATION_MAX_SESSIONS`, every one of them counting all live sessions in its scope — or the project's `automation_paused` is set. When it says no, the tick is **skipped and logged at `info`** with the bound that refused. It is not queued, not retried before the next occurrence and not remembered.

This is deliberately the same mechanism the dispatcher is bounded by, and it is the whole answer to "what if the previous run is still going": a scan that takes longer than its period piles up against its profile's `max_concurrent` and stops firing until it drains. Setting `max_concurrent` to 1 is how a user says "never two of these at once", in the setting that already means that.

Rejected: **a dedicated rule of at most one live scheduled session per profile**, a second, invisible cap with the same purpose as one the profile already has, and one that cannot express "at most three". Rejected: **always launch**, ignoring the caps for schedules on the grounds that a tick is rare — which is exactly the loop ADR 0042 exists to prevent: a minute-by-minute schedule against a slow agent would launch containers until the host gave out, and the per-project pause would not stop it. A refused tick being logged at `info` rather than `debug` is the difference from the dispatcher's no-work outcome: the dispatcher having nothing to do is ordinary, a schedule the user asked for not running is something they should be able to find in the log.

### The prompt is a required `schedule_prompt` column

A profile with a schedule has a `schedule_prompt`: a column on `agent_profiles`, required when a schedule is set and meaningless without one. `system_prompt` says who the agent is; `schedule_prompt` says what this run does. It is the `message` of the launch — which is what an ephemeral launch without a task must have (`validate_launch_prompt`) — so a scheduled launch goes down the one existing launch path with nothing special about it.

Rejected: **a `profile_schedules` table** carrying several schedules per profile, each with its own expression and prompt. It is the more general shape and buys nothing v2 needs: the case it serves — the same agent with two different jobs — is two profiles, which is already how one agent with two jobs is expressed everywhere else here, and which keeps `max_concurrent`, the served states and the pause meaning one thing per row. A table also makes the UI a list editor instead of two fields on the profile form. Rejected: **a fixed generated message** ("your scheduled run starts now"), which makes every scheduled agent's instructions live in its `system_prompt`, where the profile is also the thing a user might launch by hand, and leaves no way to say what *this* schedule is for. Requiring the column rather than defaulting it is the point: a scheduled agent that was never told what to do is a mistake worth refusing at save.

### `last_scheduled_at` is an exactly-once guard, not a catch-up cursor

`agent_profiles.last_scheduled_at` is written in the same transaction that decides a tick fires, before the launch. A restart inside the tick's minute therefore cannot fire the same tick twice, and a crash between the write and the launch loses that one run — which is the same outcome as the outage rule above, and the direction this trades in deliberately: one run lost rather than one run doubled.

It is not a cursor to catch up from. Nothing walks the occurrences between its value and now looking for work to do; the job asks only whether an occurrence falls in the window it is looking at, and several occurrences in one window are one firing. The distinction is written down because the column looks exactly like a cursor, and reading it as one would quietly reintroduce the catch-up behaviour rejected above.

### Eligibility and attribution are ADR 0042's, unchanged

A schedule may be set on `ephemeral` profiles only, and is refused at save unless the profile backend's agent credential resolves without a user, at `global` or `project` scope (ADR 0036). The credential can be deleted afterwards, so the job checks again and skips such a profile with an `info` line. A scheduled session has `created_by` NULL, `launch_source` `schedule` and the tracker actor `System`. None of this is restated in the scheduler's own section; it refers to "Unattended launches".

## Consequences

- The scheduler is a `cron/` job over the same capacity function and the same launch path as the dispatcher, with no task claim: what is new is the expression, the `schedule_prompt` and the `last_scheduled_at` write.
- `agent_profiles` gains a schedule expression, `schedule_prompt` and `last_scheduled_at`; the schema task carries them and `docs/data-model.md` with them, together with the tick granularity — one minute — that makes an expression finer than a minute meaningless.
- The API refuses a schedule without a `schedule_prompt`, a schedule on a non-`ephemeral` profile and a schedule whose credential does not resolve, the last two exactly as it already refuses `auto_launch` (`SPEC.md`, "Agent profiles").
- Storing no time zone means a schedule cannot express "9 am wherever the team is". That is accepted for v2; if it is ever needed, a per-profile zone is an added column and a changed evaluation, not a changed model.
- A user whose instance was down over a weekend sees no sessions for it, and no way to ask for the missed ones. Launching the profile by hand from the project page, with the same message, is the answer.
- `croner` is the first crate here with a proc-macro dependency added for parsing alone (`derive_builder`, `strum`); the cost is build time in a build that already has several.
