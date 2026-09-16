# 0017. Separate upstream tracking from Mars integration branches

Status: accepted. Supersedes ADR 0007's ref-ownership and pruning rationale; orchestrator-only remote writes and ADR 0001's reference clones remain in force.

## Context

The project repository serves both as a cache of upstream history and as the place Mars integrates session work before pushing it. Fetching upstream heads directly into `refs/heads/*` conflates those roles: a scheduled fetch can replace an unpushed local merge with the older upstream head. Session reference clones solve working-copy isolation and disk sharing, but do not solve branch ownership. An ordinary temporary clone also does not copy custom session refs or the source repository's remote-tracking refs.

Options considered:

1. Keep exact mirror semantics and always push immediately after merging. Rejected: merge and push are separate user actions, and an upstream failure must not discard a successful local merge.
2. Separate upstream tracking from Mars-owned refs in the same bare repository. Chosen: retains object sharing and makes ownership explicit.
3. Replace reference clones with worktrees. Rejected: does not solve ref ownership and restores the shared writable git metadata problem from ADR 0001.

## Decision

- Keep a reference clone per session and the read-only project repository mount.
- Initialize the project repository as bare, without mirror-mode fetch or push. Fetch upstream heads into `refs/remotes/origin/*`, and tags into `refs/tags/*`.
- Seed integration heads under `refs/heads/*` on initial import. Later upstream fetches never move or prune them. Session work remains under `refs/sessions/*`.
- Expose upstream-tracking names such as `origin/main` separately from integration names such as `main`. The default session base remains the integration head named by the project's `default_branch`. Explicit merge/rebase operations incorporate upstream changes.
- Explicitly fetch selected custom and upstream-tracking refs into temporary operation clones; resolve session bases to commits instead of assuming `clone --branch` accepts every supported ref.
- Write back and push only explicit selected refs. A rejected push preserves local refs and any successful local merge.
- Serialize orchestrator git mutations per project across cron, REST and MCP, including dependent fetches and write-back. This lock does not govern the agent's own checkout commands.

## Consequences

- A background fetch cannot erase the branch reference to an unpushed merge.
- "Mirror" remains a shorthand in operation names and configuration; the repository is not an exact upstream mirror.
- Integrating upstream changes is an explicit action. A fresh launch's fetch refreshes upstream tracking, not the default integration head.
- The branch-list DTO distinguishes integration, upstream and session refs. Git tests must cover an unpushed merge surviving fetch, explicit session-ref access, and a rejected push retaining local work.
