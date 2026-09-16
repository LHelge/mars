# 0017. Separate upstream tracking from Mars integration branches

Status: accepted. Supersedes ADR 0007's ref-ownership and pruning rationale; orchestrator-only remote writes and ADR 0001's reference clones remain in force.

## Context

The project repository is both a cache of upstream history and the place Mars integrates session work before pushing. Fetching upstream heads straight into `refs/heads/*` conflates the two: a scheduled fetch can replace an unpushed local merge with the older upstream head. Reference clones solve working-copy isolation, not branch ownership.

Options considered:

1. Keep exact mirror semantics and push immediately after every merge. Rejected: merge and push are separate user actions, and an upstream failure must not discard a successful local merge.
2. Separate upstream tracking from Mars-owned refs in the same bare repository. Chosen: keeps object sharing and makes ownership explicit.
3. Replace reference clones with worktrees. Rejected: does not solve ownership and reintroduces the shared writable metadata problem from ADR 0001.

## Decision

- The project repository is bare, without mirror-mode fetch or push. Upstream heads go to `refs/remotes/origin/*`; integration heads under `refs/heads/*` are seeded on import and never moved or pruned by later fetches; session work stays under `refs/sessions/*`.
- Upstream names (`origin/main`) are exposed separately from integration names (`main`). The default session base is the integration head; upstream changes enter through explicit merge or rebase.
- Write-back and push touch only explicitly selected refs; a rejected push preserves local work.
- Orchestrator git mutations are serialized per project. The lock does not govern the agent's own checkout.

## Consequences

- A background fetch cannot erase an unpushed merge.
- "Mirror" survives as shorthand in operation names; the repository is not an exact upstream mirror.
- Integrating upstream changes is explicit. A fresh launch's fetch refreshes upstream tracking, not the integration head.
