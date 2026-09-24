Every project has a task board that agents and people share. A task's state says which queue it is waiting in, and whoever holds the task is working on it. Agents move tasks with their task tools, and you do it on the board. Both follow the rules below, except that a person is never bound by a lease.

### States and their kinds

The board's columns are the project's own states, and you can edit them on the **States** tab. Each state has a kind:

| Kind | Meaning | New project |
| --- | --- | --- |
| Queue | Agents pick work up here. A profile's served states are queue states. | `backlog`, `ready`, `review`, `merge` |
| Human | Where tasks land that need a person. There is exactly one. | `needs_human` |
| Terminal | Closes the task and satisfies what depends on it. | `done`, `cancelled` |

A project keeps at least one queue state and one terminal state. Renaming a state renames it everywhere at once, including in the profiles that serve it. A state that still holds tasks can't be deleted, and neither can a state that is another state's conflict state.

With the starter profiles, a task goes like this. The planner turns a `backlog` item into tasks in `ready`. The implementer claims one, does the work and hands it to `review`. The reviewer approves it on to `merge`, or sends it back to `ready` with what has to change. In `merge`, Mars merges the approved work into the default branch and moves the task to `done` by itself. Any of these steps can be yours instead.

### Auto-merge states

A queue state can be an **auto-merge** state. The **States** tab turns it on per state with the **Auto-merge** box, and asks for a **Conflict state**: another queue state, `ready` by default. The board marks such a column with an `auto-merge` chip. A new project's `merge` state is one.

When an approved task arrives in an auto-merge state, Mars merges the commit the reviewer approved into the project's default branch, usually within a second. No agent and no session is involved. A fast-forward is used when the default branch hasn't moved since the work started, and a merge commit otherwise. The task then moves to the project's first terminal state, `done` in a new project, with a comment naming the commit it merged. That closes it and unblocks whatever depended on it.

- **A conflict** leaves the default branch untouched. The task goes to the conflict state with a comment listing the conflicting paths, one per line. Its hand-off stays the approved one, so the next implementer starts from it, brings it up to date and publishes a new revision, which is reviewed again.
- **A task without an approved hand-off** can only have been moved in by hand. Mars doesn't merge it and sends it to the human state instead, with the reason.
- **A held task** is left alone. A profile that serves an auto-merge state keeps the tasks it claims there.
- **Pausing automation**, in the project's **Settings**, pauses auto-merge too. Tasks wait in the state until you turn it off ([Automation](help:automation)).

To keep one task from being merged, move it out of the state before you approve it, or turn auto-merge off. Merged work stays on Mars's default branch until someone pushes it ([Branches and merging](help:branches)).

### Held tasks

There is no "in progress" column. A task is being worked on in whatever state it is in, and its card shows the session holding it. That hold is the task's **lease**. An agent takes it with `claim`, and only one session can hold a task at a time. The lease lasts as long as the session is alive, and a parked session still counts. When the holder moves the task to another state, the lease goes with the move. When the holder gives up, it releases the task, which keeps its state. When a session ends, Mars releases what it held and says so in a comment.

You can move, release and edit any task, held or not. **Release** gives a held task back without moving it.

### Attempts and escalation

**Attempts** counts the claims since the task last changed state. When a task is released with its attempts at the project's **Max attempts** (3 by default, set in the project's **Settings**), it goes to the human state instead of back into its queue, with the reason recorded. Three agents failing on one task give you one task in `needs_human` with their three comments, not a fourth try. Moving a task to another state resets its attempts. A review loop therefore isn't capped by attempts, because each send-back is a move. Rounds cap it instead.

**Rounds** count the revisions published since the task last left the human state: one for each trip through an implementer, whatever your states are called. A card shows its round once it passes one, as `round 2/5`. A **send-back** is a review that requests changes, or an auto-merge that conflicts. When an agent or Mars sends a task back and its rounds have reached the project's **Max rounds** (5 by default, set in the project's **Settings**), the task goes to the human state instead, with `round limit reached` and the send-back's comment as the reason. A send-back you make yourself on the board is never redirected, because you have already decided. Moving a task out of the human state starts its rounds from zero again.

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

In an auto-merge state, approved work merges itself, as described above. **Merge approved hand-off**, in the task's hand-off section, is for the cases auto-merge doesn't cover: a state without auto-merge, a project whose automation is paused, or a merge into another integration head than the default branch. It merges the commit that was approved, not the tip of the branch it came from. The button is only enabled while the current hand-off is approved. The merge leaves the task where it is, so you move it to `done` afterwards. A `merger` agent, if you add one, does both itself.

Here is an example. An implementer hands a task to `review` at commit A. The reviewer approves A and forwards it to `merge`. Meanwhile the implementer's branch moves on to B. The merge, automatic or yours, still merges A. To include B, publish a new revision at B and have it reviewed. A rejection forwards A to `ready` with **Request changes**, and the next implementer starts from A and publishes a new revision when it's fixed.

A merge on the project's **Branches** panel is a plain git merge and approves nothing. [Branches and merging](help:branches) covers those.
