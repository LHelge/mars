# 0053. Roll an integration head back by a revert commit, never a reset, and only for users

Status: accepted (Bears epic `ny9yq`, task `zhcgs`).

## Context

Recovering from work that was merged into the default branch but is bad — typically work built on the wrong base — took git on the host and a lot of task clean-up by hand: find the last good commit, roll the branch back, find every task merged since, move those tasks out of their terminal state, and make sure their next launch does not start from the old hand-off. Mars already records what that needs (auto-merge comments, `Requested-By:` trailers, `task_handoffs.commit`), and epic `ny9yq` makes it one guided action: `POST /projects/{pid}/git/revert` (`SPEC.md`, "Git"; `ARCHITECTURE.md`, "Git model", Revert).

Two things had to be decided: how the head goes back, and who may ask.

## Decision

**Revert, never reset.** "Revert to here" writes one new commit on top of the integration head whose tree is the chosen commit's tree (`git commit-tree <to>^{tree} -p <head>`), and moves the head forward to it with a compare-and-swap. The head never moves backwards.

**Users only.** The revert, and the task reopening that can go with it, is a REST endpoint and nothing else: no MCP tool, no profile permission, no automatic trigger. Rolling the default branch back is a human decision.

Rejected:

- **Resetting the head to the chosen commit** (`git update-ref refs/heads/<branch> <to>`). ADR 0050's end-of-session judgement relies on integration heads only moving forward: a session ref deleted because the default branch contained its tip is never re-examined, so a reset that took that tip back off the branch would lose the session's work silently. A reset also makes the next push a force push upstream, and the push action refuses a non-fast-forward unless forced.
- **Exposing the revert over MCP**, behind a profile permission as `merge` and `push` are. An agent that decides the default branch is wrong can say so in a task comment or escalate; discarding merged work, and reopening every task behind it, is not a call to delegate.
- **A general git editor in the UI** — interactive rebase, dropping individual commits, resetting arbitrary refs. History rewriting belongs in a conversational session's own checkout and comes back through the ordinary merge path, where it is reviewed like any other change.

## Consequences

- The history keeps the bad commits and the revert that took them back, which is what a reader of the default branch needs to see. A later re-merge of fixed work is an ordinary merge.
- Pushing the revert is the existing push action, fast-forward, with no `force`.
- A revert names no session, so it records no `git` event; it is described by its response and by the history.
- Reopening runs under the same project git lock as the revert, in one tracker transaction; if it fails after the head moved, the revert stays and the caller gets 500, as a failed push never rolls a merge back.
