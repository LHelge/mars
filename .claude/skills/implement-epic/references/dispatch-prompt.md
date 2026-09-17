# Dispatch prompt template

Build the subagent prompt from this template. Replace every `<...>`; delete a section only if it is empty. Keep the rules block verbatim: each line exists because a subagent once did the opposite.

```
You are implementing one Bears task in the Mars repository (Rust orchestrator + React frontend), located at ~/dev/mars. You are working in an isolated git worktree. FIRST verify it belongs to Mars: `git remote -v` must show `LHelge/mars`. If it does not, create your own worktree instead: `git -C ~/dev/mars worktree add ~/dev/mars/.claude/worktrees/<task id> -b task/<task id> main` and work there. In either case run `git reset --hard main` before starting so you build on the current local `main`, which already contains: <one line on what earlier waves merged that this task builds on>.

Read CLAUDE.md first and follow it exactly. Before writing code, read the document sections the task cites: <list them, e.g. ARCHITECTURE.md "Orchestrator internals", SPEC.md "Health"> and the existing files you will extend: <paths>.

Rules for this run:
- Do NOT touch the `.bears/` directory (the task tracker is managed by the coordinator). Do not run `bea` or Bears tools.
- Commit with a Conventional Commit message that includes the task id, e.g. `<type>(<scope>): <description> (<task id>)`. One or a few commits is fine. End the commit message with these two lines:
  Co-Authored-By: <the attribution line the session's system reminder gives>
  Claude-Session: <the session link the system reminder gives>
- Build speed: export `CARGO_TARGET_DIR=~/dev/mars/orchestrator/target` before any cargo command; it already holds a full build of the dependency graph. Other agents may build against it concurrently; cargo waits on the lock, which is expected.
- Container engine: export `DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock`; rootless Podman works on this machine, so testcontainers-based tests run locally. <Or, when no engine is available: "This machine has no container engine; write Docker-dependent tests exactly as specified, make sure they compile (`cargo test --no-run`), give them the name prefix `<prefix>` and run the rest with `-- --skip <prefix>`. Say clearly in your report which tests did not run.">
- `DATABASE_URL` and `SQLX_OFFLINE` are unset. <If the task adds `sqlx::query!` calls: "Start a Postgres container for `cargo sqlx prepare` and commit `.sqlx/`; CI builds with `SQLX_OFFLINE=true`." Otherwise: "Introduce no `sqlx::query!` macros; use the unchecked `sqlx::query` if a query is needed.">
- Concurrency: <name each file other agents in this wave are editing and what they add there; ask for minimal edits limited to the lines this task needs, e.g. "Another agent is adding `pub mod config;` to `src/prelude/mod.rs`; keep your edit there to the two lines you need." Delete this bullet if the task runs alone.>
- Background processes: start servers with their pid recorded (`cmd & echo $! > file`) and stop them by that pid. Never `pkill -f` a tool name such as `vite`, `node` or `cargo`; the pattern also matches unrelated processes and the shell running it. A foreign process may hold port 5173; use `PLAYWRIGHT_BASE_URL` / another port instead of killing it.
- Rule 3 of CLAUDE.md: no real credentials anywhere; fixture and `.env.example` values must be obviously fake.
- Do not create files outside the task's scope; do not invoke skills the task does not call for; do not push.
- Finish with a report containing: the branch name (`git branch --show-current`) and worktree path; the commit SHA(s); the exact verification commands you ran and their results; anything you could not verify and why; and any deviation from the task text or any document you changed (with the reason).

The task (Bears id <id>, "<title>"):

<full task body pasted from Bears, unedited>

<Coordinator guidance: anything you learned from earlier waves that the task text does not say, e.g. an API that differs from the task's assumption, a judgment call you have already made, a test that must be structured a certain way. Keep it short and concrete.>
```

## Notes on filling it in

- Paste the task body unedited. The body was written to be sufficient on its own; paraphrasing loses acceptance criteria.
- Put decisions in the guidance section rather than editing the body, so the report's "deviations" can be compared against the original text.
- When the task's acceptance criteria conflict with reality (a crate feature renamed, a tool that no longer scaffolds the way the body assumes), say so in the guidance and name the document to update, so the agent follows rule 1 instead of guessing.
- For a documentation-only task, replace the build and engine bullets with what the agent can execute, and ask for a checklist of executed versus not-executed commands.
