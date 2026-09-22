Every project has a task board that agents and people share. A task's state says which queue it is waiting in, and whoever holds the task is working on it. Agents move tasks with their task tools, and you do it on the board. Both follow the rules below, except that a person is never bound by a lease.

### States and their kinds

The board's columns are the project's own states, and you can edit them on the **States** tab. Each state has a kind:

| Kind | Meaning | New project |
| --- | --- | --- |
| Queue | Agents pick work up here. A profile's served states are queue states. | `backlog`, `ready`, `review`, `merge` |
| Human | Where tasks land that need a person. There is exactly one. | `needs_human` |
| Terminal | Closes the task and satisfies what depends on it. | `done`, `cancelled` |

A project keeps at least one queue state and one terminal state. Renaming a state renames it everywhere at once, including in the profiles that serve it. A state that still holds tasks can't be deleted.

With the starter profiles, a task goes like this. The planner turns a `backlog` item into tasks in `ready`. The implementer claims one, does the work and hands it to `review`. The reviewer approves it on to `merge`, or sends it back to `ready` with what has to change. The `merger` profile merges the approved work into the default branch and moves the task to `done`. Any of these steps can be yours instead.

### Held tasks

There is no "in progress" column. A task is being worked on in whatever state it is in, and its card shows the session holding it. That hold is the task's **lease**. An agent takes it with `claim`, and only one session can hold a task at a time. The lease lasts as long as the session is alive, and a parked session still counts. When the holder moves the task to another state, the lease goes with the move. When the holder gives up, it releases the task, which keeps its state. When a session ends, Mars releases what it held and says so in a comment.

You can move, release and edit any task, held or not. **Release** gives a held task back without moving it.

### Attempts and escalation

**Attempts** counts the claims since the task last changed state. When a task is released with its attempts at the project's **Max attempts** (3 by default, set in the project's **Settings**), it goes to the human state instead of back into its queue, with the reason recorded. Three agents failing on one task give you one task in `needs_human` with their three comments, not a fourth try. Moving a task to another state resets its attempts. A review loop therefore isn't capped by attempts, because each send-back is a move.

An agent can also send a task to a person directly with its `needs_human` tool. Each escalation emails the task's assignee, or every administrator when nobody is assigned. You can turn these emails off on your own settings page. Moving a task into the human state by hand sends no email.

Tasks in the human state are listed on the [Dashboard](/). No agent picks them up by itself. To hand one back, comment with what you decided and either move it to a queue state or launch a session for it from the task.

### Blocked tasks

A task is **blocked** while it has an open child or depends, through a `blocks` dependency, on a task that isn't closed. The other dependency kinds, `discovered_from` and `related`, are only notes. A blocked task can't be claimed or launched for. It unblocks by itself when the last of those closes. A parent closes itself, into the first terminal state, when its last open child closes.

### Hand-offs

A state change can carry a **hand-off**: a pointer to committed work, with a comment on what was done and what comes next. It is how code moves from one agent to the next. A move without one is fine for planning work, and it leaves any existing hand-off as it was.

- **Publish revision** names a session and a full 40-character commit id. Mars syncs that session's branch first, and the branch tip must equal the commit you named. Uncommitted changes are never included. Mars then keeps that exact commit, so later commits on the branch don't change what was handed off.
- **Approve**, **Request changes** and **Forward without decision** pass the current hand-off on to another state, with or without a review decision. Forwarding doesn't replace the work with the reviewer's own branch.

A new revision always starts unreviewed. An earlier approval stays in the task's history, but it never carries over to new code.

A session launched for a task with a hand-off starts from the hand-off's commit, unless you choose another base ref.

### Merging reviewed work

**Merge approved hand-off**, in the task's hand-off section, merges the commit that was approved, not the tip of the branch it came from. The button is only enabled while the current hand-off is approved. The merge leaves the task where it is, so you move it to `done` afterwards. A `merger` agent does both itself.

Here is an example. An implementer hands a task to `review` at commit A. The reviewer approves A and forwards it to `merge`. Meanwhile the implementer's branch moves on to B. The task merge still merges A. To include B, publish a new revision at B and have it reviewed. A rejection forwards A to `ready` with **Request changes**, and the next implementer starts from A and publishes a new revision when it's fixed.

A merge on the project's **Branches** panel is a plain git merge and approves nothing. [Branches and merging](help:branches) covers those.
