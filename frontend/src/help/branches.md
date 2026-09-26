Mars keeps its own copy of each project's repository, the project's mirror, and every session works in a clone of it. Agents never push. Their work reaches the mirror when Mars copies it in, and it reaches your remote only when someone pushes it. The branches in the mirror come in three kinds, and each kind is moved by different things.

### Three kinds of branch

| Kind | Looks like | What it is | What moves it |
| --- | --- | --- | --- |
| Integration head | `main` | Mars's own branch. Sessions start from it and merges land in it. | A merge, a rebase or a revert, nothing else. |
| Upstream-tracking | `origin/main` | What your remote had at the last fetch. | **Fetch now**, the periodic background fetch, and every fresh launch. |
| Session ref | `refs/sessions/<id>` | One session's committed work. | **Sync**, the end of the session, and any merge, rebase or push that uses it. |

The project's **default branch** names an integration head. It is what a session starts from when you give no other base, and it is the target a merge suggests. Fetching never moves an integration head, so a merge waiting in `main` to be pushed survives any number of fetches, even if the remote moved or deleted its branch in the meantime.

Inside a session's clone the names look different. The clone's `origin` is Mars's mirror, so `origin/main` there is Mars's `main`, not your remote's. A session branch is called `session/<id>`.

### Taking in upstream changes

A fetch brings your remote's new commits into `origin/main`, but `main` stays where it was. To bring them into Mars's branch, merge `origin/main` into `main`. The **Merge any ref** form on the project's **Branches** tab, under the integration heads, is set up for exactly that. A session launched afterwards starts from the combined `main`. If the upstream and Mars's changes conflict, nothing is changed and you get the list of conflicting paths.

### Getting work out of a session

A session's agent commits in its own clone. **Sync** copies the session's committed work into the mirror as its session ref. Uncommitted changes stay behind, so an agent that wants its work to travel commits it first. Ending a session syncs it too, except that a session whose work is already kept somewhere else ends with no session ref at all, and leaves the **Branches** tab: one that made no commits of its own, such as a planner or a reviewer that only passed a hand-off on, and one whose every commit a hand-off or an integration head such as `main` already holds, such as an implementer that handed off its last commit. Its **Changes** still show what it did (nothing, for the first kind), with a line saying it has no branch of its own. Work that only your remote's branches or tags hold does not count, because a fetch can remove those, so such a session keeps its ref. From there, each session branch on the project's **Branches** tab, and a session's own row behind the **branch** toggle of its header, offers:

- **Merge into…**: merge the session's work into an integration head, such as `main`.
- **Rebase onto…**: replay the session's commits onto an integration head or an upstream-tracking branch.
- **Push…**: publish the session branch to a branch on your remote.

A rebase rewrites the session branch in the mirror. A running session's checkout follows it only if it has no uncommitted changes. Otherwise the checkout is left alone, and the session's git event reports that it needs reconciling, which is the agent's job. Merges and rebases stop at a conflict and leave the branch they would have written untouched. Mars never resolves a conflict by itself, but it can hand one to an agent.

### Resolving a conflict

When a merge from the **Branches** tab or a session's **branch** panel conflicts, the conflicting paths are listed under the form with **Resolve with an agent** beneath them. It opens a small launch form: a conversational profile, preselected as the one named `resolver` if the project has one ([Agent profiles](help:profiles) offers it as a template) and otherwise the default profile, and a first message Mars writes for you, naming the branch you tried to merge, its commit, the conflicting paths and how to fetch it. Edit the message if the agent needs more to go on, then launch.

The session starts from the branch you were merging into. The agent fetches the other branch, merges it into its own, resolves the conflicts, runs the project's checks and commits. It never merges into your branch itself: when it says it is ready, its session branch holds both sides and the resolution, and you merge that branch with **Merge into…** like any other.

### Deleting a session

A session ref lives no longer than its session. Deleting an ended session deletes its ref from the mirror too, and its branch leaves the **Branches** tab. Before you confirm, the delete says how many commits the branch has that the default branch doesn't, because those are lost with it unless they were merged, pushed or handed off first. A branch you pushed to your remote stays there; Mars never deletes a remote branch.

### Pushing

A push sends exactly one branch to one branch on the remote, using the project's [git credential](help:git-credential). Nothing else is pushed along with it. If your remote has moved on since the last fetch, the push is rejected and everything in Mars stays as it was. Fetch, bring the remote's changes in by merging `origin/<branch>` into the integration head or rebasing the session branch onto it, and push again. **Force push** overwrites the remote branch instead, and commits that only the remote had are lost.

After a push to GitHub, Mars shows an **Open compare on GitHub** link for opening a pull request against the default branch. Pushing the default branch to its own name has nothing to compare, so it shows none.

Merged work waits on its integration head until someone pushes it. That includes everything auto-merge lands: in a new project, each approved task is merged into Mars's default branch by itself, and none of it reaches your remote until it is pushed. To publish it, open the project's **Branches** tab and use **Push…** on the head's row under **Integration heads**. The remote branch defaults to the head's own name, so `main` goes to the remote's `main`. After you merge `origin/main` into `main`, this is the push that follows. An agent can push too if its profile grants the `push` tool, but the starter profiles don't ([Agent profiles](help:profiles)).

### Rolling back

The **History** section of the **Branches** tab, under the integration heads, lists the default branch's history one commit per row, newest first: a task merge is one row, with the task and the session its work came from. Another head can be picked from the **Head** select. **Load older** reads further back.

**Revert to here** on a row puts the branch back to that commit's content. It never moves the branch backwards: it adds one new commit on top whose files are those of the chosen commit, so nothing is lost, the undone commits stay in the history, and pushing it needs no force. After a revert its commit is the new top row, highlighted, and every row it undid is drawn faded with **undone by** and the revert's commit, which takes you to that row. An undone row offers no **Revert to here**, and neither does a row whose content is already the branch's, marked **current content**: going back there would change nothing, and Mars refuses it. Before you confirm, it lists every commit it undoes and the tasks they brought in. Tick **Reopen these tasks** to move those of them that are closed to a state you choose, with a comment saying why. Each also loses its current hand-off, so its next launch starts from the reverted branch instead of from the work you just rolled back, and the hand-off stays in its history. If the branch moved while you were reading, nothing happens and you are asked to reload the history and choose again. Like a merge, the revert waits on the branch until you push it with **Push…**.

### Branch merges and task merges

Merging a branch is a plain git action. It doesn't approve anything, and it doesn't move any task. Reviewed task work is merged from the task instead: by Mars itself when the task reaches an auto-merge state such as `merge`, or with **Merge approved hand-off**. Either way exactly the commit that was approved is merged, even if the session's branch has moved on since. [Task flow](help:task-flow) explains hand-offs, approval and auto-merge.
