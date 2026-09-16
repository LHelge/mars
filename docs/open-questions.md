# Open questions

Everything the brief left undecided that has not yet been resolved. Each entry states the options, a recommendation, and whether the other documents already assume the recommendation. Resolve an entry by moving its outcome into the relevant document (and an ADR if an alternative was rejected) and deleting it here.

The first review pass (2026-09-15) resolved 30 questions; their outcomes are in `SPEC.md`, `ARCHITECTURE.md`, `docs/data-model.md`, `README.md` and ADRs 0013 and 0014. The task-structure planning session (2026-09-16) resolved the last one, how task assignment is modelled, in ADR 0016.

The core v1 decisions are recorded. The items below remain to be verified against the pinned Claude Code version during the adapter task, including the MCP-loading interaction in item 4. Each is answered by the live probe test described in `ARCHITECTURE.md`, "Input encoding", or by the engine tests, and the answer is written back into the document that depends on it; the item is then deleted here.

## Verification items

1. The stdin user-message shape under `--input-format stream-json` (`{"type":"user","message":{...}}`), and whether `session_id` or `parent_tool_use_id` must be present on it.
2. Whether a message written mid-turn interrupts or queues, and whether a `control_request` with subtype `interrupt` exists so that a turn can be stopped without parking the session.
3. Whether any prompt-like message (AskUserQuestion, a permission request) can reach the host under `--permission-mode bypassPermissions --permission-prompts none`. If not, the `prompt` event kind and the `answer` input are removed from `SPEC.md`.
4. Verify `--strict-mcp-config` against the pinned version; it is listed in the [current CLI reference](https://code.claude.com/docs/en/cli-reference). Before enabling it, resolve its interaction with the documented repository-owned `.mcp.json` behavior: strict mode ignores that configuration as well as other discovered MCP servers.
5. Whether `result.total_cost_usd` and `result.usage` are per turn or cumulative for the process, which sets the accumulation rule in `ARCHITECTURE.md`, "Cost accounting".
6. Whether `system`/`init` includes `model` and `tools`; only `session_id` and `mcp_servers` are documented.
7. Whether `SIGINT` sent with `kill` reaches the CLI as PID 1 on both engines without `Init: true`.
8. Whether `UsernsMode: "keep-id:uid=1000,gid=1000"` is accepted through Podman's compatibility API.
9. Verify multi-turn stdin with `--print`, nested subagent text with `--forward-subagent-text`, and changed profile prompts on resume with `--system-prompt-snapshot off` using the complete launch command in `ARCHITECTURE.md`, "Claude Code invocation".
