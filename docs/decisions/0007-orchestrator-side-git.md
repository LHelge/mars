# 0007. Remote git operations only in the orchestrator, via MCP

Status: accepted

## Context

Agents produce commits. Somebody has to get them upstream. Letting the agent push directly is the fewest moving parts, but it means the container holds a credential that can write to the remote, and merges happen wherever the agent decides, unaudited. A future "merge agent" role would need the same capabilities under the same controls.

## Decision

Session containers hold no git credentials and never push. They commit to their local `session/<id>` branch only. The orchestrator:

- fetches a session's branch into the project mirror on completion or on demand: `git fetch /data/sessions/<id>/work session/<id>:refs/sessions/<id>`;
- exposes `list_session_branches`, `merge`, `rebase` and `push` as MCP tools, restricted per profile through `agent_profiles.mcp_tools`, and as REST endpoints for the UI;
- records every merge, rebase and push as a `TaskEvent`-like audit row and a session event, attributed to the calling session or user.

Session branches live under `refs/sessions/<id>` in the mirror, not `refs/heads/session/<id>`, because the mirror's fetch refspec would otherwise prune them as "not on origin". When pushed upstream they become `refs/heads/session/<id>`.

## Consequences

- One code path for humans and agents; every remote write is attributed and logged.
- A future merge agent is a profile with `merge`, `rebase`, `push` in its `mcp_tools` and nothing else.
- Conflicts during `merge`/`rebase` are reported to the caller as structured errors; the orchestrator never leaves the mirror in a conflicted state (operations run in a temporary clone of the mirror, see `ARCHITECTURE.md`, "Git model").
- Agents cannot pull upstream changes into their session on their own; a `rebase` request is the way to bring a session branch up to date, and the result is fetched back into the session work tree by the orchestrator.
