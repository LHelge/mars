# Open questions

Everything the brief left undecided that has not yet been resolved. Each entry states the options, a recommendation, and whether the other documents already assume the recommendation. Resolve an entry by moving its outcome into the relevant document (and an ADR if an alternative was rejected) and deleting it here.

The first review pass (2026-09-15) resolved 30 questions; their outcomes are in `SPEC.md`, `ARCHITECTURE.md`, `docs/data-model.md`, `README.md` and ADRs 0013 and 0014. The task-structure planning session (2026-09-16) resolved the last one, how task assignment is modelled, in ADR 0016.

**Nothing is open.** The verification items that remained were the ones only the real Claude Code CLI could answer, and the live probe (`orchestrator/tests/claude_probe.rs`) answered all of them against the pinned version on 2026-09-18. Its observations are in `orchestrator/tests/fixtures/claude/2.1.274/NOTES.md` with the recordings beside them, and the rules they establish are in the documents that own them:

- the stdin line shape, that neither `session_id` nor `parent_tool_use_id` is needed on it, that a mid-turn message queues rather than interrupts, and that a `control_request`/`interrupt` exists and behaves like `SIGINT` — `ARCHITECTURE.md`, "Input encoding";
- that no prompt or permission question can reach the host under `--permission-mode bypassPermissions --permission-prompts none`, so the `prompt` event and the `answer` input do not exist — ADR 0033, with the contract in `SPEC.md`, "WebSocket: session stream" and "AgentEvent";
- that `--strict-mcp-config` does suppress the repository's own `.mcp.json` servers and is therefore left off, and that an unreachable server is reported as `failed` without failing the turn — `ARCHITECTURE.md`, "Claude Code invocation";
- that `result.total_cost_usd` is cumulative for the process while `usage` is per turn — `ARCHITECTURE.md`, "Cost accounting";
- that `system`/`init` carries `model` and `tools` (and much else Mars ignores), and that nothing on it reports a resume — `ARCHITECTURE.md`, "Claude Code invocation"; `SPEC.md`, "AgentEvent", the `init` rule;
- that multi-turn stdin, `--forward-subagent-text` and a changed profile prompt on `--resume` with `--system-prompt-snapshot off` all behave as ADR 0003 assumes — `ARCHITECTURE.md`, "Claude Code invocation";
- that the CLI writes nothing until its first stdin line, which is why a session is `running` on stdin attach — ADR 0032, with the sequence in `ARCHITECTURE.md`, "Launch sequence";
- that `kill` reaches PID 1 without an init process and that `keep-id` works through Podman's compat API — `ARCHITECTURE.md`, "Session image" and "Session container specification".

Every finding is version-specific: it holds for the CLI version pinned in `images/claude/Dockerfile` (`ARG CLAUDE_CODE_VERSION`), and the probe re-checks it on a bump. A new question is added back to this file under a heading of its own.
