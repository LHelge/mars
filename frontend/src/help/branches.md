Mars keeps its own copy of each project's repository, the project's mirror, and every session works in a clone of it. Agents never push. Their work reaches the mirror when Mars copies it in, and it reaches your remote only when someone pushes it. The branches in the mirror come in three kinds, and each kind is moved by different things.

### Three kinds of branch

| Kind | Looks like | What it is | What moves it |
| --- | --- | --- | --- |
| Integration head | `main` | Mars's own branch. Sessions start from it and merges land in it. | A merge or a rebase, nothing else. |
| Upstream-tracking | `origin/main` | What your remote had at the last fetch. | **Fetch now**, the periodic background fetch, and every fresh launch. |
| Session ref | `refs/sessions/<id>` | One session's committed work. | **Sync**, the end of the session, and any merge, rebase or push that uses it. |

The project's **default branch** names an integration head. It is what a session starts from when you give no other base, and it is the target a merge suggests. Fetching never moves an integration head, so a merge waiting in `main` to be pushed survives any number of fetches, even if the remote moved or deleted its branch in the meantime.

Inside a session's clone the names look different. The clone's `origin` is Mars's mirror, so `origin/main` there is Mars's `main`, not your remote's. A session branch is called `session/<id>`.

### Taking in upstream changes

A fetch brings your remote's new commits into `origin/main`, but `main` stays where it was. To bring them into Mars's branch, merge `origin/main` into `main`. The **Merge any ref** form on the project's **Branches** tab, under the integration heads, is set up for exactly that. A session launched afterwards starts from the combined `main`. If the upstream and Mars's changes conflict, nothing is changed and you get the list of conflicting paths.

### Getting work out of a session

A session's agent commits in its own clone. **Sync** copies the session's committed work into the mirror as its session ref. Uncommitted changes stay behind, so an agent that wants its work to travel commits it first. Ending a session syncs it too. From there, each session branch on the project's **Branches** tab, and a session's own row behind the **branch** toggle of its header, offers:

- **Merge into…**: merge the session's work into an integration head, such as `main`.
- **Rebase onto…**: replay the session's commits onto an integration head or an upstream-tracking branch.
- **Push…**: publish the session branch to a branch on your remote.

A rebase rewrites the session branch in the mirror. A running session's checkout follows it only if it has no uncommitted changes. Otherwise the checkout is left alone, and the session's git event reports that it needs reconciling, which is the agent's job. Merges and rebases stop at a conflict and leave the branch they would have written untouched. Mars never resolves a conflict for you.

### Pushing

A push sends exactly one branch to one branch on the remote, using the project's [git credential](help:git-credential). Nothing else is pushed along with it. If your remote has moved on since the last fetch, the push is rejected and everything in Mars stays as it was. Fetch, bring the remote's changes in by merging `origin/<branch>` into the integration head or rebasing the session branch onto it, and push again. **Force push** overwrites the remote branch instead, and commits that only the remote had are lost.

After a push to GitHub, Mars shows an **Open compare on GitHub** link for opening a pull request against the default branch. Pushing the default branch to its own name has nothing to compare, so it shows none.

Merged work waits on its integration head until someone pushes it. To publish it, open the project's **Branches** tab and use **Push…** on the head's row under **Integration heads**. The remote branch defaults to the head's own name, so `main` goes to the remote's `main`. After you merge `origin/main` into `main`, this is the push that follows. An agent can push too if its profile grants the `push` tool, but the starter profiles don't ([Agent profiles](help:profiles)).

### Branch merges and task merges

Merging a branch is a plain git action. It doesn't approve anything, and it doesn't move any task. Reviewed task work is merged from the task instead, with **Merge approved hand-off**, which merges exactly the commit that was approved, even if the session's branch has moved on since. [Task flow](help:task-flow) explains hand-offs and approval.
