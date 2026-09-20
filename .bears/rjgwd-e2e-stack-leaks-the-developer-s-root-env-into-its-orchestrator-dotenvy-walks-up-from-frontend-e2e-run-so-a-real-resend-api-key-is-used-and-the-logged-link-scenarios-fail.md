---
id: rjgwd
title: "E2E stack leaks the developer's root .env into its orchestrator: dotenvy walks up from frontend/.e2e/run, so a real RESEND_API_KEY is used and the logged-link scenarios fail"
status: open
priority: P1
created: "2026-09-20T23:58:18.585720151Z"
updated: "2026-09-20T23:58:18.585720151Z"
tags:
  - frontend
  - e2e
  - orchestrator
  - bug
---

## Summary
Found on 2026-09-21 on a developer machine whose repository-root `.env` sets `RESEND_API_KEY` (a real key, for trying the compose stack). `npm run test:e2e` failed exactly the five scenarios that read an invitation, reset or escalation link out of the orchestrator's log (`auth.spec.ts:226`, `:282`, `:323`, `helpers.spec.ts:76`, `task-sessions.spec.ts:502`) with `no invite link for … in …/.e2e/orchestrator.log within 15000 ms`; the other 87 passed, and CI (no `.env`) is green.

Cause, from reading the code (not yet confirmed by a run with the key removed): `tests/e2e-stack.sh` starts the orchestrator with `env -i` from `frontend/.e2e/run` and its comment says `Config::from_env`'s `.env`/`../.env` lookup "finds nothing". But `Config::load_dotenv` calls `dotenvy::from_filename(".env")`, and that function searches the current directory **and every parent**, so it finds `<repo>/.env`. Variables the stack does not set itself come from it — `RESEND_API_KEY` and `MAIL_FROM` among them — so the orchestrator uses `ResendClient`, nothing is logged, and **the run attempts real sends through the developer's Resend account** to the suite's `@example.test` addresses.

## Documents
- `README.md` "Development", "End-to-end tests"
- `frontend/tests/README.md`
- `ARCHITECTURE.md`/`README.md` wherever the `.env` lookup order is described, if the lookup itself changes

## Acceptance criteria
- [ ] Decide where to fix, and do at least the first:
  - **The stack**: make the e2e orchestrator immune to any `.env` — e.g. run it from a directory outside the repository (`mktemp -d`), or pass the variables that select behaviour explicitly empty (`RESEND_API_KEY=`, `MAIL_FROM=`, `SECRETS_MASTER_KEY_FILE=`), provided `Config` treats empty as unset (check `value()`), since `dotenvy` never overrides a variable that is already set.
  - **The lookup**: `load_dotenv` documents "`.env`, then `../.env`" but actually walks all ancestors. Make the code match the document (`dotenvy::from_path` on the two explicit paths) — this also makes a host-run orchestrator's behaviour predictable.
- [ ] A check in `e2e-stack.sh up` (or a Playwright global setup assertion) that the running orchestrator uses the logging email client — e.g. assert on a startup log line — so a leak fails fast with a clear message instead of as five timeouts.
- [ ] The five scenarios pass on a machine whose root `.env` sets `RESEND_API_KEY`.
- [ ] A unit test for the `.env` lookup order if the lookup changes.

## Edge cases
- Other leaked variables are possible too (`SESSION_NETWORK_*`, `RUST_LOG`, `HTTP_PORT` are harmless; `SECRETS_MASTER_KEY_FILE` would override the stack's fresh key). List what the stack must pin.
- Rule 3: the fix must not print the key while diagnosing.

## Testing
- Reproduce first: root `.env` with `RESEND_API_KEY=re_fake_not_a_real_key` and `MAIL_FROM=mars@example.invalid`, run `auth.spec.ts`; expect the failure before the fix and a pass after.
