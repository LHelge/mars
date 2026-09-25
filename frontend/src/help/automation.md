Mars does three things in a project with nobody asking it to. The **dispatcher** launches sessions for tasks as they arrive, a **schedule** runs a profile on the clock, and **auto-merge** merges approved work into the default branch.

The first two start sessions, and both are settings of an ephemeral profile. Only an ephemeral profile can run unattended, because nobody is there to answer a conversational one. A session nobody launched carries a `dispatcher` or `schedule` tag wherever it is listed. Auto-merge starts no session at all. It is a setting of a task state, not of a profile.

### Launching for tasks as they arrive

Tick **Let the dispatcher launch this profile** in the profile's **Unattended launches** section. A new project's `implementer` and `reviewer` come with it ticked, so a task put in `ready` is implemented, reviewed and, through auto-merge, merged with nobody launching anything, once the credential below is stored. The dispatcher then watches the states the profile serves and launches a session of it for a task that can be claimed there. It picks the same task the agent's `ready` tool would offer first: highest priority first, then the lowest task number. The launch works like yours from a task: the task is claimed in the same step, the agent is told which task it holds, and the session starts from the task's current hand-off, if it has one.

The dispatcher reacts to the board, so a task moved into a served state is normally picked up within a second. A timer sweeps behind it in case a change was missed. A few more rules:

- It only takes tasks in the profile's served states. A task that is held, blocked, in the human state or closed is never dispatched, so a task escalated to a person waits for that person.
- When two profiles serve the same state, the older profile gets the task.
- A launch that fails releases its task, and the failure counts as an attempt. After `max_attempts` of them the task goes to the human state, which is the only back-off there is ([Task flow](help:task-flow)).

### Scheduled runs

Tick **Run this profile on a schedule** and give it a cron expression and a prompt. Each time the expression comes due, a session of the profile starts with the prompt as its message and no task, from the project's default branch. It is titled after the profile and the time it ran. The profile's system prompt says who the agent is, and the schedule prompt says what this run does.

The expression has five fields: minute, hour, day of month, month and day of week. It is **always read in UTC**, whatever your own time zone. A seconds field, a year field and forms like `@daily` are refused. For example:

| Expression | Runs |
| --- | --- |
| `0 4 * * *` | every day at 04:00 UTC |
| `*/30 * * * *` | every 30 minutes |
| `0 7 * * 1-5` | weekdays at 07:00 UTC |

The profile editor shows the next and the last run once you have saved, and the **Profiles** tab marks the profile with a `schedule` chip.

**A tick is never caught up.** A run that came due while Mars was down is skipped, not replayed when it comes back. A tick refused by a cap, by the pause or by a missing credential is spent too. The next occurrence is the retry. A schedule means "run at these times", not "run this many times". The `tech-debt-scanner` template, offered under **Start from** when you add a profile, is a ready-made example that runs once a day.

### Merging approved work

A queue state with **Auto-merge** on, such as a new project's `merge` state, is merged by Mars itself. When a reviewer approves a task and moves it there, Mars merges the approved commit into the default branch and closes the task, normally within a second. A merge that conflicts sends the task to the state's conflict state with the conflicting paths. You turn it on or off per state on the **States** tab, and [Task flow](help:task-flow) has the details.

Auto-merge launches nothing, so the caps below don't apply to it and it needs no agent credential. Only the project's pause stops it.

### The three caps

Three limits hold unattended launches back. A launch happens only while every one of them has room:

| Cap | Where it is set | Default |
| --- | --- | --- |
| Live sessions of this profile | The profile's **Unattended launches** section | 1 |
| Live sessions in this project | The project's **Settings** | No cap |
| Live sessions on the instance | `AUTOMATION_MAX_SESSIONS`, set by whoever runs Mars | 4 |

A live session is one that is creating or running, whoever launched it, so your own sessions use up room too. A parked session doesn't count. The caps only ever hold automation back: a session you launch by hand is never refused by one. A launch the caps refuse is skipped, not queued. The dispatcher simply tries again once a session ends, while a scheduled tick is spent. Setting a scheduled profile's cap to 1 is how you say "never two of these at once".

### Pausing a project

**Pause automation**, in the project's **Settings**, stops everything above in the project until you turn it off: the dispatcher, the schedules and auto-merge. The project header says it is paused. Nothing is caught up afterwards: the dispatcher simply picks up the tasks that are waiting, approved tasks in an auto-merge state are merged then, and scheduled ticks that came due meanwhile are skipped. Sessions that are already running carry on, and you can still launch sessions and merge approved hand-offs by hand. To stop a single profile instead, untick its dispatcher or schedule setting. To stop merging in one state, turn its auto-merge off.

### The credential unattended runs use

An unattended session has no person behind it, so a credential stored for **Me** can't reach it. It needs an [agent credential](help:agent-credentials) stored for the project or for **Everyone**. Until one exists, the starter implementer and reviewer simply wait: their tasks stay unclaimed, and you can launch either by hand with **Run once**. A profile can't be saved with the dispatcher or a schedule turned on until one exists either, and that includes an edit to the starter implementer or reviewer: store the credential, or untick the dispatcher setting to save without it. If that credential is deleted later, the profile's runs are skipped and claim nothing until one is back.
