# 0007. Remote git operations only in the orchestrator, via MCP

Status: accepted

Superseded by [0017](0017-separate-upstream-and-integration-refs.md) for ref ownership and pruning rationale. Remote writes remain orchestrator-only.

## Context

Agents produce commits that must reach upstream. Letting the agent push directly is the fewest moving parts, but the container then holds a credential that can write to the remote, and merges happen wherever the agent decides, unaudited. A future "merge agent" role needs the same capabilities under the same controls.

## Decision

Session containers hold no git credentials and never push. They commit to their local `session/<id>` branch only. The orchestrator:

- fetches a session's branch into the project repository on completion or on demand;
- exposes `list_session_branches`, `merge`, `rebase` and `push` as MCP tools, restricted per profile through `agent_profiles.mcp_tools`, and as REST endpoints for the UI;
- records every merge, rebase and push as an audit row and a session event, attributed to the calling session or user.

Session branches live under `refs/sessions/<id>` in the project repository, not `refs/heads/session/<id>`; when pushed upstream they become `refs/heads/session/<id>`.

## Consequences

- One code path for humans and agents; every remote write is attributed and logged.
- A future merge agent is a profile with `merge`, `rebase` and `push` in its `mcp_tools` and nothing else.
- Conflicts are reported as structured errors; operations run in a temporary clone so the project repository is never left conflicted (`ARCHITECTURE.md`, "Git model").
- Agents cannot pull upstream changes on their own; a `rebase` request brings a session branch up to date and the orchestrator fetches the result back into the session work tree.
