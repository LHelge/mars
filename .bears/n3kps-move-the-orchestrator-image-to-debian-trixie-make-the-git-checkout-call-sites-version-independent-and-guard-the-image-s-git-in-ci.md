---
id: n3kps
title: Move the orchestrator image to Debian trixie, make the git checkout call sites version-independent and guard the image's git in CI
status: done
priority: P0
created: "2026-09-19T17:06:10.849575672Z"
updated: "2026-09-19T17:47:18.458807158Z"
tags:
  - orchestrator
  - infra
  - git
parent: "5czwa"
attempts: 1
---

## Summary
Found by the Podman walkthrough (rmu9n): on the packaged orchestrator image every session launch fails, and `/git/merge` and `/git/rebase` would too. The image is `debian:bookworm-slim`, whose git is 2.39.5; `git checkout` on that version does not understand `--end-of-options`, treats it and the commit after it as pathspecs, and `-b`/`-B` plus a pathspec is fatal:

```
the session clone failed: git checkout --quiet -b session/<uuid> --end-of-options <sha> failed (exit 128):
fatal: Cannot update paths and switch to branch 'session/<uuid>' at the same time.
```

Reproduced inside the image: `git checkout --quiet -b x --end-of-options <sha>` → 128; `git checkout --quiet -b x <sha>` → 0; `git merge ... --end-of-options <sha>` → 0. CI is green because every git test runs the runner's newer git, never the image's. The epic's first acceptance criterion (a stub session runs from the README walkthrough) only passed with a local, reverted one-line patch, and `README.md` "Running it" now states that a stub session runs, so this task makes that sentence true.

## Scope change (2026-09-19, decided by the user)
The orchestrator image moves from bookworm to trixie: bookworm is ageing, and `debian:trixie-slim` ships git 2.47.3, which accepts `checkout -b/-B <branch> --end-of-options <sha>` (measured, rc 0). The call-site fix and the 2.39.5 audit stay, because a host-run orchestrator uses the host's git, which can be bookworm's.

## Documents
- `ARCHITECTURE.md` "Git model" (the wrapper, option-injection protection), "Orchestrator internals" (startup `git` probe), "Trust boundaries" (the image ships `git`); ADR 0011 (git is the binary), ADR 0012.
- `orchestrator/Dockerfile` (runtime `debian:bookworm-slim`, builder `rust:<version>-bookworm`).
- `README.md` "Prerequisites"/"Development" → "Deployment images" (names the base images), "CI" table.
- `CLAUDE.md` "Testing expectations" (git tests use real repositories; git is never mocked).

## Acceptance criteria
- [ ] `orchestrator/Dockerfile`: runtime `debian:trixie-slim`, builder `rust:<version>-trixie`, same Debian release in both stages, move-together comment kept. Everything the Dockerfile task (c9u6c) proved still holds: builds with podman, under 200 MB, `git --version` through `--entrypoint git`, the `--read-only --tmpfs /tmp` healthcheck smoke exits 1 with `healthcheck: connection refused`, a foreign uid (`--user 4242:4242`) runs, no curl/wget.
- [ ] The three failing call sites use a version-independent form and keep their protection against a revision that looks like an option: `orchestrator/src/git/session.rs` (session work clone, `checkout -b`), `orchestrator/src/git/integrate.rs` (merge worktree and rebase worktree, `checkout -b`/`-B`). Suggested shape: `checkout --quiet -b <branch> <commit> --`; a comment says why `--end-of-options` is not used for `checkout`.
- [ ] Audit: every other git subcommand the orchestrator passes `--end-of-options` to is exercised against git 2.39.5 and either proven to work or fixed the same way. The evidence (subcommand, argv shape, result on 2.39.5) goes in the report, and the minimum supported host git the audit supports is documented where the startup git probe or the prerequisites are described.
- [ ] Regression guard in CI: the crate's git tests run against the git the image ships, inside the builder image the Dockerfile names (`rust:<version>-trixie`), printing `git --version`; filtered to the test binaries that need neither Postgres nor an engine. It lives in the Orchestrator CI workflow (runs on `orchestrator/src/**` changes) and the `README.md` "CI" table says so. A second leg on bookworm is added only if 2.39 is documented as the floor.
- [ ] Every mention of bookworm that is about the orchestrator image (`grep -rn bookworm`) is updated: README "Deployment images", Dockerfile comments, `deploy.yml`, ADR/ARCHITECTURE if they name it. `images/claude/Dockerfile` (`node:22-bookworm-slim`, the session image) is out of scope.
- [ ] The backend chain passes; actionlint passes.

## Implementation notes
- Do not bring a compose stack up for this task (a sibling task uses the fixed `mars-*` network names on this machine this round); the packaged end-to-end proof is the Docker walkthrough task (kdzzm), which depends on this one.

## Testing
- The git tests inside the builder container; the call-site failure reproduced on 2.39.5 before the fix and gone after; the image smoke checks; the full backend chain on the host.

## Documentation
- `ARCHITECTURE.md` "Git model", `README.md` "Deployment images" and "CI", same commit.

Discovered by rmu9n.