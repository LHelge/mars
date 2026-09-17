---
id: "5hv6b"
title: Verify the README Development walkthrough and CLAUDE.md commands against the skeleton and write back drift
status: done
priority: P2
created: "2026-09-16T20:30:11.463488902Z"
updated: "2026-09-17T05:50:44.322187019Z"
tags:
  - docs
  - infra
depends_on:
  - apjkw
  - bpwq5
  - mv5w9
  - xxufa
  - fxyx2
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Walk the `README.md` "Development" section and the `CLAUDE.md` "Code quality" commands from a clean checkout against the finished skeleton and fix every sentence that no longer matches: the "Nothing in this repository is implemented yet" preamble, the intended layout tree, the local-run commands, the CI table, and the `ARCHITECTURE.md` crate table if a crate name or feature changed during scaffolding. This closes the epic's documentation obligation (rule 1) in one reviewable commit rather than scattering edits.

## Documents
- `README.md`: intro paragraph ("Nothing in this repository is implemented yet"), "Development" (layout tree, "Running locally", "Orchestrator", "Frontend"), "CI".
- `ARCHITECTURE.md` "Orchestrator internals" (tree and Crates table).
- `CLAUDE.md` "Backend conventions", "Frontend conventions", "Code quality", "Running locally".
- `docs/open-questions.md` (verify no entry is affected; none is expected).

## Acceptance criteria
- [ ] From a fresh clone, following `README.md` "Running locally" verbatim (Postgres container, `DATABASE_URL`, `cp ../.env.example ../.env`, `cargo run`) yields an orchestrator answering `GET /api/health` with 200; every command that failed or needed an undocumented step is fixed in the README.
- [ ] `cd frontend && npm install && npm run dev` proxies `/api/health` as the README says; the `npm run test:e2e` prerequisites (browser install, `PLAYWRIGHT_*` variables) are documented.
- [ ] The README intro no longer states that nothing is implemented; replace with one sentence saying the skeleton exists and the documents describe the system being built.
- [ ] The README "Development" layout tree matches the repository (`orchestrator/`, `frontend/`, `.github/workflows/`, `.env.example`; `images/`, `nginx/`, `compose.yml` stay listed as intended until their epics land, marked "(planned)" or left as is with a note).
- [ ] The `ARCHITECTURE.md` crate table matches `orchestrator/Cargo.toml` exactly (names and features); any crate added during scaffolding that is not in the table is either removed from `Cargo.toml` or added to the table with its concern.
- [ ] The `CLAUDE.md` "Code quality" command chains run green as written on the skeleton; if a flag had to change (for example `--all-targets`), update the chain there and in the CI workflows together.
- [ ] `docs/open-questions.md` is unchanged (no scaffolding item exists there) — confirm and state it in the PR.

## Implementation notes
- Files: `README.md`, `ARCHITECTURE.md`, `CLAUDE.md` (only if a command changed), possibly `.env.example` comments.
- Do the walkthrough on Linux with Podman as the README assumes; note in the PR which engine was used. The engine is not needed for the health endpoint in this epic (`engine_ready` is the placeholder), so a machine without Podman can still complete the backend steps.
- Keep edits minimal and factual; no restructuring of the documents.

## Edge cases
- If `cargo run` from `orchestrator/` does not pick up `../.env` (the config task added the fallback), the README `cp ../.env.example ../.env` line is correct; otherwise fix whichever side is wrong, code preferred.
- `DATA_DIR_HOST` must be set in `.env` for `Config::from_env()` to succeed on the host; the README's "Orchestrator" paragraph already says so — make sure `.env.example` has a usable placeholder (`./data`) so the walkthrough does not fail.

## Testing
- Manual walkthrough recorded in the PR description as a checklist of the README commands with outcomes.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` and `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e` pass unchanged.

## Documentation
- This task *is* the documentation write-back: `README.md` and, if needed, `ARCHITECTURE.md` and `CLAUDE.md` change in the same commit.

## Assumes from other epics
- Deployment packaging epic verifies the compose-based "Running it" section; this task covers only "Development".