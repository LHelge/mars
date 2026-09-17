---
id: kjvte
title: Implement the ContainerSpec builder from the session container specification table
status: done
priority: P0
created: "2026-09-16T20:27:13.946656503Z"
updated: "2026-09-17T16:33:46.693365621Z"
tags:
  - orchestrator
  - engine
depends_on:
  - "84dxt"
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement `orchestrator/src/engine/spec.rs`: the single place that turns the "Session container specification" table into a `ContainerSpec` and, on the bollard side, into `bollard::container::Config` + `HostConfig`. The launcher (Session lifecycle epic) supplies only the variable inputs (ids, image, runtime, command, secrets, shared directories); every fixed value (labels, user, workdir, stdin flags, security options, root filesystem, user namespace rule) is set here and nowhere else, so the epic's criterion "no `HostConfig` field outside the documented table is used" is enforced by construction and by a unit test.

## Documents
- `ARCHITECTURE.md` "Session container specification" (the whole table and the "Uid contract" and "Development on the host" paragraphs)
- `ARCHITECTURE.md` "Storage" (mount paths: `DATA_DIR_HOST` sources, `DATA_DIR` targets for the mirror and CLI state dir, shared directories at their container path)
- `ARCHITECTURE.md` "Engine adapter" (`UsernsMode` only on Podman; nested bind mounts, parents before children)
- `ARCHITECTURE.md` "Networks" (created on `SESSION_NETWORK_INTERNAL`)
- `README.md` "Configuration" (`DATA_DIR`, `DATA_DIR_HOST`, `SESSION_NETWORK_INTERNAL`, `SESSION_NETWORK_EGRESS`, `SESSION_EXTRA_HOSTS`)
- `docs/data-model.md` `agent_profiles` (`image`, `runtime`)
- ADR 0004, ADR 0015

## Acceptance criteria
- [ ] `SessionSpecInput` + `build_session_spec(&SessionSpecInput) -> ContainerSpec` produce, for a fresh session, exactly: image `profile.image`; name `mars-session-<sid>`; labels `mars.session_id`, `mars.project_id`, `mars.profile_id`; user `1000:1000`; working dir `/session/work`; cmd = the given launch command; env in the order `HOME=/session/home`, `CLAUDE_CONFIG_DIR=<DATA_DIR>/projects/<pid>/claude`, `MARS_SESSION_ID=<sid>`, `MARS_PROJECT_ID=<pid>`, `MARS_TASK_ID=<tid>` (only when `task_id` is `Some`), then the secrets in the order given; binds `<DATA_DIR_HOST>/sessions/<sid>/work → /session/work` rw, `…/home → /session/home` rw, `…/log → /session/log` rw, `…/mcp.json → /session/mcp.json` ro, `<DATA_DIR_HOST>/projects/<pid>/repo.git → <DATA_DIR>/projects/<pid>/repo.git` ro, `<DATA_DIR_HOST>/projects/<pid>/claude → <DATA_DIR>/projects/<pid>/claude` rw, then one `<DATA_DIR_HOST>/projects/<pid>/shared/<name> → <container_path>` rw per shared directory; binds ordered so that every parent target precedes any child target; network = `SESSION_NETWORK_INTERNAL`; extra hosts = parsed `SESSION_EXTRA_HOSTS`; runtime = `profile.runtime`; `open_stdin = true`.
- [ ] `build_probe_spec(&ProbeSpecInput) -> ContainerSpec` produces the same fixed fields (user, workdir, security, networks, extra hosts, root fs, userns) with the probe's own name `mars-probe-<random>`, label `mars.probe=true`, the three probe binds (`<probe_dir_host>/work → /session/work`, `…/home → /session/home`, `…/log → /session/log`, all rw), `open_stdin = false`, `cmd = ["sh", "-c", "touch /session/work/probe-ok"]`, and no `MARS_*` env.
- [ ] `to_bollard(&ContainerSpec, EngineKind) -> (bollard::container::Config<String>, bollard::models::HostConfig)` sets only: `Config { image, hostname: None, labels, user, working_dir, cmd, env ("K=V"), open_stdin, stdin_once: Some(false), tty: Some(false), attach_stdin: Some(open_stdin), attach_stdout: Some(false), attach_stderr: Some(false), host_config }` and `HostConfig { binds, network_mode, extra_hosts, cap_drop: ["ALL"], security_opt: ["no-new-privileges"], privileged: Some(false), readonly_rootfs: Some(false), runtime, userns_mode: Some("keep-id:uid=1000,gid=1000") only when EngineKind::Podman }`. A unit test serialises the `HostConfig` to JSON and asserts the key set equals exactly that list (minus `Runtime`/`UsernsMode` when unset), so any new field fails the test until the table is updated.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- File: `orchestrator/src/engine/spec.rs`. Public items: `SessionSpecInput`, `ProbeSpecInput`, `build_session_spec`, `build_probe_spec`, `to_bollard`, `parse_extra_hosts`, `order_binds`.
- `SessionSpecInput { session_id: Uuid, project_id: Uuid, profile_id: Uuid, task_id: Option<Uuid>, image: String, runtime: Option<String>, cmd: Vec<String>, secrets: Vec<(String, String)>, shared_dirs: Vec<SharedDirMount { name: String, container_path: String }>, data_dir: PathBuf, data_dir_host: PathBuf, network_internal: String, extra_hosts: Vec<String> }`. The caller has already validated shared-directory paths through the model (absolute, normalised, not under `DATA_DIR`, not equal to or an ancestor of the four session paths); the builder does not re-validate but debug-asserts absoluteness.
- `parse_extra_hosts(raw: &str) -> Vec<String>` splits on commas, trims, drops empties; each entry is passed to the engine verbatim as `host:ip` (`host.containers.internal:host-gateway`). Config parsing may already do this in the scaffolding epic; if so, call that and delete the duplicate.
- `order_binds` sorts by container target path component count ascending, then lexicographically, keeping it stable: `/session/work` before `/session/work/target`, `/session/work/target` before `/session/work/target/debug`. Document that the engine mounts in list order and that the engine tests (later task) verify nested mounts on both engines.
- Bind string format for bollard `Binds`: `<host_source>:<container_target>:ro` or `:rw`. Host source paths are `DATA_DIR_HOST`-based and must be absolute; the mirror and CLI state dir targets are `DATA_DIR`-based (ADR 0001: the reference clone's alternates file records the orchestrator's path).
- `CLAUDE_CONFIG_DIR` is set here as the table specifies, even though it is Claude-specific; a second backend would add a builder input rather than move this out.
- Keep `ContainerSpec` free of bollard types; `to_bollard` is the only function in `spec.rs` that imports `bollard`.

## Edge cases
- `task_id: None` must omit `MARS_TASK_ID` entirely (not set it empty).
- A secret named like a fixed variable (`HOME`) is appended after the fixed ones; the engine takes the last value. Do not dedupe silently: return `EngineError::Unsupported("secret name collides with a reserved variable: HOME")` for names in `{HOME, CLAUDE_CONFIG_DIR, MARS_SESSION_ID, MARS_PROJECT_ID, MARS_TASK_ID}` so the launcher records a clear `sessions.error`.
- Empty `runtime` string from a profile is treated as `None`.
- `extra_hosts` empty → `HostConfig.extra_hosts = None`, not `Some(vec![])`, to leave the engine default untouched.
- Windows-style or relative `data_dir_host` is a programming error; `debug_assert!` and document that `Config::from_env()` makes it absolute.

## Testing
- Unit tests in `spec.rs`: full-fixture assertion of `build_session_spec` (compare against a literal `ContainerSpec`); `MARS_TASK_ID` presence/absence; secret ordering after fixed env; reserved-name collision error; `order_binds` on `/session/work/target`, `/session/work`, `/session/home`, `/session/work/target/debug`; `to_bollard` key-set test via `serde_json::to_value(host_config).as_object().keys()`; `userns_mode` present for `Podman`, `None` for `Docker`; `parse_extra_hosts` trimming and empties.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written. If a field has to be added, update the "Session container specification" table first and reference the engine-test verification in the "Engine adapter" table (rule in `ARCHITECTURE.md`).

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `Config` exposes `data_dir`, `data_dir_host` (absolute), `session_network_internal`, `session_network_egress`, `session_extra_hosts`.
- "Projects, agent profiles and shared directories": shared-directory path validation lives on the model there; this builder trusts its inputs.
