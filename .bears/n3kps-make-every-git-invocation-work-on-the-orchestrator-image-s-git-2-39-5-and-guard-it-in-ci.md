---
id: n3kps
title: Make every git invocation work on the orchestrator image's git 2.39.5 and guard it in CI
status: open
priority: P0
created: "2026-09-19T17:06:10.849575672Z"
updated: "2026-09-19T17:06:10.849575672Z"
tags:
  - orchestrator
  - infra
  - git
parent: "5czwa"
---

## Summary
Found by the Podman walkthrough (rmu9n): on the packaged orchestrator image every session launch fails, and `/git/merge` and `/git/rebase` would too. The image is `debian:bookworm-slim`, whose git is 2.39.5; `git checkout` on that version does not understand `--end-of-options`, treats it and the commit after it as pathspecs, and `-b`/`-B` plus a pathspec is fatal:

```
the session clone failed: git checkout --quiet -b session/<uuid> --end-of-options <sha> failed (exit 128):
fatal: Cannot update paths and switch to branch 'session/<uuid>' at the same time.
```

Reproduced inside the image: `git checkout --quiet -b x --end-of-options <sha>` → 128; `git checkout --quiet -b x <sha>` → 0; `git merge ... --end-of-options <sha>` → 0. CI is green because every git test runs the runner's newer git, never the image's. The epic's first acceptance criterion (a stub session runs from the README walkthrough) only passed with a local, reverted one-line patch, and `README.md` "Running it" now states that a stub session runs, so this task makes that sentence true.

## Documents
- `ARCHITECTURE.md` "Git model" (the wrapper, option-injection protection), "Orchestrator internals" (startup `git` probe), "Trust boundaries" (the image ships `git`); ADR 0011 (git is the binary).
- `orchestrator/Dockerfile` (runtime `debian:bookworm-slim`, git 2.39.5; builder `rust:<version>-bookworm`, same Debian release and so the same git).
- `README.md` "Prerequisites"/"Development" if a minimum git version is stated or needs stating; "CI" table if a job is added.
- `CLAUDE.md` "Testing expectations" (git tests use real repositories; git is never mocked).

## Acceptance criteria
- [ ] The three failing call sites work on git 2.39.5 and keep their protection against a revision that looks like an option: `orchestrator/src/git/session.rs` (session work clone, `checkout -b`), `orchestrator/src/git/integrate.rs` (merge worktree and rebase worktree, `checkout -b`/`-B`). Suggested shape: `checkout --quiet -b <branch> <commit> --` (revision before a trailing `--`), with the revision already validated as a full object id or a verified ref where the code has that guarantee; say in a comment why `--end-of-options` is not used for `checkout`.
- [ ] Audit: every other git subcommand the orchestrator passes `--end-of-options` to (`fetch`, `clone`, `init`, `config`, `rev-parse`, `for-each-ref`, `update-ref`, `symbolic-ref`, `ls-remote`, `merge`, `merge-base`, `diff`, `push`, and whatever else `grep -rn end-of-options orchestrator/src` shows) is exercised against git 2.39.5 and either proven to work or fixed the same way. The evidence (subcommand, argv shape, result on 2.39.5) goes in the report.
- [ ] Regression guard in CI: the crate's git tests run against the same git the image ships. Preferred shape: a job that runs the git-related tests (`cargo test --features integration-tests` filtered to the git modules and the git integration test binaries, no database or engine needed) inside the `rust:<version>-bookworm` container the Dockerfile's builder uses, since it carries bookworm's git 2.39.5; the job prints `git --version`. It belongs in the Orchestrator CI workflow (it must run on `orchestrator/src/**` changes) and the `README.md` "CI" table says so. Before the fix the job fails on the checkout call sites; after it, it passes. Prove both locally with `podman run` of that image.
- [ ] If a minimum git version is a documented contract anywhere (`README.md`, `ARCHITECTURE.md`, the startup probe), it says 2.39 or the statement is added where the probe is described; do not raise the image's git just to avoid the fix (bookworm's git is the supported floor because it is what the image ships).
- [ ] The backend chain passes; actionlint passes.

## Implementation notes
- Files: `orchestrator/src/git/session.rs`, `orchestrator/src/git/integrate.rs`, any other call site the audit finds, `.github/workflows/orchestrator.yml` (or the workflow that owns backend CI), `README.md` "CI", `ARCHITECTURE.md` "Git model" if it states the `--end-of-options` rule as universal.
- A quick way to run one git form on 2.39.5: `podman run --rm --entrypoint git docker.io/library/debian:bookworm-slim ...` needs git installed; the `rust:<version>-bookworm` image already has it.
- Do not bring a compose stack up for this task (a sibling task uses the fixed `mars-*` network names on this machine this round); the packaged end-to-end proof is the Docker walkthrough task, which depends on this one.

## Testing
- The git tests inside the bookworm container, red before and green after; the full backend chain on the host.

## Documentation
- `ARCHITECTURE.md` "Git model" and `README.md` "CI", same commit, where they state the changed rule or the new job.

Discovered by rmu9n.