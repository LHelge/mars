---
id: "9wxhs"
title: "Implement the session launcher: mirror fetch, base-ref clone, secrets resolution, container spec assembly, start, stdin attach and owner spawn for fresh and resume launches"
status: open
priority: P1
created: "2026-09-16T20:31:36.800460115Z"
updated: "2026-09-16T20:31:36.800460115Z"
tags:
  - orchestrator
  - sessions
  - engine
  - git
  - secrets
depends_on:
  - mvrfc
  - qtx4x
parent: s52qg
---

## Summary
Implement `Launcher` in `orchestrator/src/session/launcher.rs`: the asynchronous sequence that turns a `creating` or `parked` session row into a running container with an attached `SessionOwner`. It runs the documented order (mcp.json, mirror fetch with the 30-second skip and warning-on-failure, base-ref resolution and reference clone under the project git lock, secrets resolution with the both-credentials refusal, container spec assembly from the specification table, pull, create, network connect, start, stdin attach, owner spawn) and routes every failure to `failed` with `sessions.error`. Resume differs only in rotating the token, skipping git, and passing `--resume`.

## Documents
- `ARCHITECTURE.md` "Launch sequence" (sequence diagram and the paragraph after it: fresh launch fetches the mirror first, skipped if fetched < 30 s ago, fetch failure is a `launch_warning`; profile prompt with `--append-system-prompt` and `--system-prompt-snapshot off` on every launch; `--mcp-config /session/mcp.json`; fresh token per actual launch; ephemeral prompt = generated task message + user message), "Session container specification" (the complete table: image, name, labels, user, workdir, command, env order, stdin flags, binds in order with parents before children, rootfs, networks, extra hosts, security, userns, runtime), "Storage" (shared dirs created at launch, owned like the session dirs; CLI state dir per project), "Claude Code invocation" (credentials: refuse when both `ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN` resolve), "Secrets" → "Resolution at launch" (missing names → `launch_warning`; `secret_uses` rows), "Git model" → "Session clone" (resolve base, `clone --reference --no-checkout`, explicit fetch of the selected ref, `session/<sid>` at the commit, launching user's identity) and "Serialization" (fresh launch holds the project git lock through base resolution and clone setup).
- `SPEC.md` "AgentEvent" (`launch_warning { message }`, `state_change`), "Sessions" (`Session.container_id`, `error`).
- `docs/data-model.md` `sessions` (`container_id`, `error`, `mcp_token_hash`), `secret_uses` (`purpose = launch`).
- ADRs 0001, 0003, 0029.

## Acceptance criteria
- [ ] `Launcher::launch(&self, session_id: Uuid, mode: LaunchMode) ` where `LaunchMode::Fresh { token: McpToken } | Resume` spawns a tokio task and returns immediately; the task holds a registry `LaunchGuard` for the session (if `try_begin_launch` returns `None`, log and return without a second launch).
- [ ] Fresh sequence, in order: load session, project, profile, project shared dirs, launching user; `SessionDirs::ensure`; `write_mcp_json`; take the project git lock; if `projects.last_fetched_at` is older than 30 s or null, run the mirror fetch and on failure append `launch_warning { message: "mirror fetch failed: <git message>" }` and continue; resolve `base_ref` to a commit (failure → fail the launch with `error = "base ref '<ref>' does not resolve"`); clone the session work tree on branch `session/<sid>` with `user.name`/`user.email` from the launching user (missing user → bot identity); release the git lock.
- [ ] Resume sequence: `rotate_token` (commit hash, then file) before anything else; require `cli_session_id` (absent → fail with `error = "session has no CLI session id to resume"` unless the work directory is missing too, in which case run the fresh sequence: this is a retry of a session that failed during creation); no git step.
- [ ] Secrets: call the secrets epic's launch resolver with `profile.secrets`, `project_id`, `created_by`, `session_id`; append one `launch_warning { message: "secret <NAME> is not defined at any scope" }` per missing name; if the resulting env contains both `ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN`, fail the launch with `error = "profile resolves both ANTHROPIC_API_KEY and CLAUDE_CODE_OAUTH_TOKEN; keep exactly one"`.
- [ ] Shared directories: for each `project_shared_dirs` row create `DATA_DIR/projects/<pid>/shared/<name>` if missing; ensure `DATA_DIR/projects/<pid>/claude` exists.
- [ ] Container spec, built from the engine epic's `ContainerSpec` builder and nothing else: image `profile.image`; name `mars-session-<sid>`; labels `mars.session_id`, `mars.project_id`, `mars.profile_id`; user `1000:1000`; workdir `/session/work`; command from `backend.launch_command(&LaunchContext { profile, mode, cli_session_id, prompt })`; env in this order: `HOME=/session/home`, `CLAUDE_CONFIG_DIR=<DATA_DIR>/projects/<pid>/claude`, `MARS_SESSION_ID=<sid>`, `MARS_PROJECT_ID=<pid>`, `MARS_TASK_ID=<task id>` only when `session.task_id` is set, then the resolved secrets; stdin `OpenStdin: true, StdinOnce: false, Tty: false, AttachStdin: true`; binds in this order: `<DATA_DIR_HOST>/sessions/<sid>/work → /session/work` rw, `…/home → /session/home` rw, `…/log → /session/log` rw, `…/mcp.json → /session/mcp.json` ro, `<DATA_DIR_HOST>/projects/<pid>/repo.git → <DATA_DIR>/projects/<pid>/repo.git` ro, `<DATA_DIR_HOST>/projects/<pid>/claude → <DATA_DIR>/projects/<pid>/claude` rw, then one rw bind per shared dir sorted so parents precede children (sort by container path length after lexical order); `ReadonlyRootfs: false`; network `SESSION_NETWORK_INTERNAL` at creation and `SESSION_NETWORK_EGRESS` connected before start; `SESSION_EXTRA_HOSTS`; `CapDrop: ["ALL"]`, `SecurityOpt: ["no-new-privileges"]`, `Privileged: false`; userns `keep-id:uid=1000,gid=1000` only when the engine is Podman; `Runtime` = `profile.runtime` when set.
- [ ] Engine steps: pull the image if absent (failure → `error = "image pull failed: <engine message>"`); create; connect egress; start; attach stdin; persist `container_id` with `set_container_id` as soon as create succeeds.
- [ ] Spawn `SessionOwner` with `start_offset = 0` (fresh) or `max_offset(sid)` (resume), `resumed = matches!(mode, Resume)`, the stdin writer and the container id; the registry entry stays `Creating` until the owner sees `init`.
- [ ] Any failure after the row is `creating` (fresh) or `parked` (resume): if a container was created, remove it (force) and clear `container_id`; `transition(current → failed, reason = "launch failed", error = <message>)`; `registry.remove(sid)` (queued inputs dropped, ADR 0020); call `on_session_failed(sid)` hook (lease release; wired by the launch-for-task task). Engine messages go into `sessions.error` verbatim; secret values never do.
- [ ] `Launcher` is reachable from `AppState` (field `launcher: Arc<Launcher>` or constructed from the state) so routes, the service and recovery share one implementation.

## Implementation notes
- Files: `orchestrator/src/session/launcher.rs`, `orchestrator/src/session/mod.rs`, `orchestrator/src/prelude/state.rs`.
- The 30-second rule reads `projects.last_fetched_at` and the mirror-fetch routine from the git epic updates it; do not duplicate the fetch code.
- Lock order: project git lock → (no DB project lock needed here) → the session row lock taken inside repository calls. Never call the engine while holding the git lock beyond the clone step.
- `LaunchContext` and `launch_command` come from the agent epic; the launcher only assembles inputs (`prompt` is `None` for conversational sessions; for ephemeral ones the launch-for-task task fills the generated message + user message; until then `prompt = session.first_message`, stored by the route on the launch call, not in the DB).
- Log at `info` with `session_id`, `project_id`, `container_id` fields only; log secret names, never values.

## Edge cases
- Resume when the container name `mars-session-<sid>` still exists (stale from a crash): remove it before create, log `warn!`.
- Fresh launch where `work/` already contains a clone (retry after a failed container step): skip the clone if `work/.git` exists and `git rev-parse --abbrev-ref HEAD` is `session/<sid>`; otherwise remove `work/` and clone again.
- Profile deleted between insert and launch (`ON DELETE RESTRICT` prevents this while the session exists): treat NotFound as a launch failure anyway.
- Orchestrator shutdown during a launch: the owner is never spawned; recovery marks the `creating` session failed on next start.

## Testing
- Integration tests in `orchestrator/tests/session_launcher.rs` with `TestApp` (mock engine, real bare repository under `tempfile` from the git epic's helpers, mock secrets values through the secrets API): fresh launch creates `work/` on `session/<sid>`, `home/`, `log/stream.jsonl`, `mcp.json`; the mock engine's recorded spec matches the table field by field (env order, bind order, labels, name, stdin flags, security, no extra `HostConfig` fields); `MARS_TASK_ID` absent without a task; a project whose `last_fetched_at` is 5 s old is not fetched again, one at 60 s is (assert through the git repo's `FETCH_HEAD` mtime or a counter on the fetch routine); an unreachable upstream yields a `launch_warning` and the launch proceeds; an unresolvable base ref fails the session with the exact error; both credentials fail the session with the exact error; a pull failure fails the session with the engine message; resume replaces `mcp_token_hash`, rewrites `mcp.json`, skips git, and the recorded command contains `--resume <cli_session_id>`; a failure after create removes the container and clears `container_id`; shared dir binds are ordered parent before child (`/session/work/target` after `/session/work`).
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Container engine adapter": `ContainerEngine` trait (pull, create, connect network, start, attach stdin, kill, remove, list by label, is_podman), the `ContainerSpec` builder and the mock engine recording specs.
- "Git operations": project git lock, mirror fetch routine, `resolve_base_ref`, `clone_session_work_tree`.
- "Secrets manager": `resolve_for_launch(...) -> (env map, missing names)` writing `secret_uses` rows.
- "Claude Code agent backend": `LaunchContext`, `AgentBackend::launch_command`.
- "Projects, agent profiles and shared directories": `Profile`, `SharedDir` repository, project data-dir layout.