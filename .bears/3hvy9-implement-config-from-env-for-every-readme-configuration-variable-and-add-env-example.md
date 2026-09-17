---
id: "3hvy9"
title: "Implement Config::from_env() for every README configuration variable and add .env.example"
status: done
priority: P0
created: "2026-09-16T20:26:39.136095593Z"
updated: "2026-09-17T05:28:46.750305854Z"
tags:
  - orchestrator
  - core
depends_on:
  - jeyrc
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add `Config` to the prelude: a struct with one typed field per variable in the `README.md` "Configuration" table, built by `Config::from_env()` which loads `.env` through `dotenvy`, applies the documented defaults, and fails fast with an error that names every missing required variable at once. Add `.env.example` at the repository root carrying exactly the same variables as the README table, with obviously fake values. Later epics read their settings from this struct and never call `std::env::var` themselves.

## Documents
- `README.md` "Configuration" (the variable table and its defaults; the "Generate a master key" line), "Development", "Running locally", "Orchestrator" (`DATA_DIR` default `./data` made absolute; `DATA_DIR_HOST` equals `DATA_DIR` on the host; `MCP_URL` on a development host).
- `ARCHITECTURE.md` "Storage" (`DATA_DIR` vs `DATA_DIR_HOST`), "Session container specification", "Development on the host" (`MCP_URL` default `http://orchestrator:<MCP_PORT>/mcp`, `SESSION_EXTRA_HOSTS` form `host:ip`), "Networks" (network name defaults), "Secrets", "Keyring" (`SECRETS_MASTER_KEYS` form `<version>=<base64 32 bytes>`, or `SECRETS_MASTER_KEY_FILE`), "MCP design" (`MCP_PORT` default 7001), "Session lifecycle", "Stop semantics" (`STOP_GRACE_SECS` default 20), "Git model" (`MIRROR_FETCH_INTERVAL_SECS` default 600).
- `CLAUDE.md` "Backend conventions": "`README.md`, 'Configuration', is the variable contract and `.env.example` carries the same variables. `Config::from_env()` fails fast naming any missing required variable." Rule 3: no real credentials in fixtures.
- ADR 0026: `RESEND_API_KEY` unset is a valid configuration (log fallback), not an error.

## Acceptance criteria
- [ ] `orchestrator/src/prelude/config.rs` defines `pub struct Config` with these fields and types:
  - required, no default: `public_url: url-ish String` (validated to start with `http://` or `https://`; expose `pub fn public_url_is_https(&self) -> bool` for the `Secure` cookie rule), `jwt_secret: String` (non-empty), `database_url: String`, `docker_host: String`, `data_dir_host: PathBuf`, `git_bot_name: String`, `git_bot_email: String`, `session_image_default: String`;
  - exactly one of `SECRETS_MASTER_KEYS` / `SECRETS_MASTER_KEY_FILE` required: stored as `secrets_master_keys: SecretsMasterKeySource` (`Inline(String)` | `File(PathBuf)`); the raw text is kept, parsing into a keyring belongs to the Secrets manager epic;
  - optional with defaults: `data_dir: PathBuf` (default `./data`, canonicalised to an absolute path at startup by resolving against the current directory; the directory need not exist yet), `mcp_url: String` (default `http://orchestrator:<mcp_port>/mcp` computed after `MCP_PORT`), `session_network_internal: String` (`mars-sessions`), `session_network_egress: String` (`mars-egress`), `session_extra_hosts: Vec<String>` (comma-separated, trimmed, empty entries dropped, each entry must contain exactly one `:`), `api_port: u16` (7000), `mcp_port: u16` (7001), `stop_grace_secs: u64` (20), `mirror_fetch_interval_secs: u64` (600), `resend_api_key: Option<String>` (`None` when unset **or empty**), `mail_from: Option<String>` (required only when `resend_api_key` is `Some`; otherwise optional), `rust_log: String` (`info`).
  - not read: `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` are compose bootstrap variables and are not fields; they still appear in `.env.example`.
- [ ] `Config::from_env() -> std::result::Result<Config, ConfigError>` calls `dotenvy::dotenv().ok()` first, then delegates to `Config::from_vars(impl Fn(&str) -> Option<String>)` (pure, testable) so unit tests never mutate process environment.
- [ ] `ConfigError` (thiserror) has variants `Missing(Vec<String>)` rendering as `missing required configuration: PUBLIC_URL, JWT_SECRET` (comma-separated, table order), `Invalid { name: String, reason: String }` rendering as `invalid configuration <NAME>: <reason>`, and `ConflictingKeys` rendering as `set exactly one of SECRETS_MASTER_KEYS and SECRETS_MASTER_KEY_FILE`. All missing variables are collected before returning; the first invalid one returns immediately after the missing check.
- [ ] `Config` derives `Debug` **manually** (or via a wrapper) so that `jwt_secret`, `database_url`, `secrets_master_keys` and `resend_api_key` print as `"<redacted>"`; a unit test asserts `format!("{config:?}")` contains none of the secret values (rule 3).
- [ ] `/.env.example` lists every variable from the README table, in table order, each with a comment line quoting the README meaning, defaults filled in, secrets as placeholders such as `JWT_SECRET=change-me-to-a-long-random-string`, `SECRETS_MASTER_KEYS=1=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=` (a base64 of 32 zero bytes, clearly fake), `RESEND_API_KEY=` left empty with a comment referencing ADR 0026.
- [ ] A unit test parses `.env.example` (path `../.env.example` from `orchestrator/`) with `dotenvy::from_path_iter`, asserts the set of keys equals the set of names the README table lists (hard-code the expected list in the test; it doubles as the contract), and asserts `Config::from_vars` over those values succeeds.
- [ ] `prelude/mod.rs` re-exports `Config` and `ConfigError`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/prelude/config.rs` (new), `orchestrator/src/prelude/mod.rs` (re-export), `/.env.example` (new, repository root as the README layout shows).
- Reading pattern: a small private helper `fn required(vars, name, missing: &mut Vec<String>) -> Option<String>` that pushes into `missing` instead of returning early, so the error lists everything at once; `fn optional_parsed<T: FromStr>(vars, name, default) -> Result<T, ConfigError>` for numbers.
- `data_dir` absolute resolution: `std::env::current_dir()?.join(path)` when relative, then lexically normalise (`.`/`..` components) without requiring existence; `DATA_DIR_HOST` is stored as given (it is a host path the engine interprets, never resolved locally).
- Do not validate `DOCKER_HOST` beyond non-empty; `bollard` parses it in the Container engine epic.
- Do not parse `SECRETS_MASTER_KEYS` entries here beyond non-empty; the keyring (Secrets manager epic) owns `<version>=<base64>` validation and the base64 length check.
- Keep `RUST_LOG` in `Config` as the string handed to the tracing initialiser in the next task.

## Edge cases
- Empty string values count as missing for required variables (a `.env` with `JWT_SECRET=` is a common mistake).
- Both `SECRETS_MASTER_KEYS` and `SECRETS_MASTER_KEY_FILE` set: `ConflictingKeys`. Neither set: appears in `Missing` as `SECRETS_MASTER_KEYS or SECRETS_MASTER_KEY_FILE`.
- `API_PORT` equal to `MCP_PORT`: `Invalid { name: "MCP_PORT", reason: "must differ from API_PORT" }`.
- `STOP_GRACE_SECS=0` is allowed (immediate SIGTERM); negative or non-numeric values are `Invalid`.
- `PUBLIC_URL` with a trailing slash is stored with the slash stripped so link building in the auth epic can append paths.
- `.env` is loaded only from the current working directory; running from `orchestrator/` with `.env` at the root (as `README.md` shows: `cp ../.env.example ../.env`) must work, so also try `dotenvy::from_filename("../.env")` when `./.env` is absent, and log at `debug` which one loaded (never the values).

## Testing
- Unit tests in `config.rs` (`#[cfg(test)]`): all-required-present succeeds with documented defaults asserted one by one; missing several variables lists all of them in table order; empty string treated as missing; conflicting key sources; `API_PORT == MCP_PORT`; `SESSION_EXTRA_HOSTS` parsing (`"a:1, b:2 ,"` → two entries; `"nocolon"` → `Invalid`); `MCP_URL` default follows a non-default `MCP_PORT`; `RESEND_API_KEY` set without `MAIL_FROM` → `Missing(["MAIL_FROM"])`; `Debug` output redaction; `.env.example` round trip.
- Command: `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `.env.example` is itself the artefact the README references. If a default discovered during implementation differs from the README table (for example the `MCP_URL` default form), fix `README.md` "Configuration" in the same commit; do not add new variables without adding them to the table.

## Assumes from other epics
- Secrets manager epic parses `SecretsMasterKeySource` into the keyring and validates entries.
- Container engine epic passes `docker_host`, network names, extra hosts and `session_image_default` to bollard.
- Authentication epic uses `jwt_secret`, `public_url`, `resend_api_key`, `mail_from`.