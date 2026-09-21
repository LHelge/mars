---
id: vbtbj
title: "Role templates: reach for `cargo binstall` first and name `cargo install` as the fallback"
status: done
priority: P2
created: "2026-09-21T11:56:49.474588004Z"
updated: "2026-09-21T12:12:47.715589675Z"
tags:
  - orchestrator
  - profiles
  - docs
depends_on:
  - f67v8
parent: qpshf
attempts: 1
---

## Summary
Discovered by the live pass `f67v8` (`images/claude-dev/VERIFY.md`, "Finding: the paragraph does not point at `cargo binstall`"). The environment paragraph added by `g8rx8` offers "`cargo install` or `cargo binstall`" as an undifferentiated pair. In scenario C the agent needed `cargo-nextest` and took the first: `cargo install cargo-nextest` locked 465 crates, outlasted the CLI's 120-second foreground Bash timeout and cost about a dozen turns of polling a background log, before the agent fell back to `cargo binstall cargo-nextest -y`, which finished in 1.36 s. `cargo-binstall` is in the dev image precisely so this does not happen (`zqe6v`), but nothing in the prompt points the agent at it.

## Documents
- `SPEC.md`, "Role profile templates" — reproduces the prompts verbatim; the byte-for-byte test compares it with the files, so both change together.
- `images/claude-dev/VERIFY.md`, "Finding" — its last sentence says the wording is a follow-up task; once fixed, say the wording was changed by this task (the observation of 2026-09-21 stays as observed; do not rewrite the "Observed" section, it was not re-run).

## Acceptance criteria
- [ ] `orchestrator/src/projects/templates/implementer.md`, `reviewer.md` and `merger.md`: the install examples name `cargo binstall` as the way to get a cargo tool, with `cargo install` as the fallback when no prebuilt binary exists or `cargo binstall` itself is absent. The paragraph still names no tool as *present* (a profile's image is editable, `g8rx8`): the wording must hold on an image without `cargo-binstall`, hence the fallback.
- [ ] Brief: the paragraph is paid for in every session's context; this is a reordering and a few words, not a new sentence about timeouts.
- [ ] One shared wording across the three roles, as today; `planner.md` untouched.
- [ ] `SPEC.md`, "Role profile templates" carries the new text verbatim and `tests/profile_templates.rs` passes; the module doc of `orchestrator/src/projects/profile_templates.rs` still describes the paragraph correctly.
- [ ] fmt, both clippy invocations, `--lib`, `--test profile_templates --test profiles --test projects_create`.

## Implementation notes
- Reaches new projects only (ADR 0038); say so in the commit body, not in the docs.
- Not re-verified live: a credentialed re-run of scenario C is not part of this task.