---
id: g8rx8
title: "Role templates: say the container is disposable and a missing tool is installed at user level, not reported as a blocker"
status: open
priority: P1
created: "2026-09-21T10:01:01.513997908Z"
updated: "2026-09-21T10:01:01.513997908Z"
tags:
  - orchestrator
  - profiles
  - docs
parent: qpshf
---

## Summary
The seeded role prompts say nothing about the environment, so an agent that finds no `cargo` stops and reports it. Add one short environment paragraph to the roles that build and test, stating what is true of every Mars session container and what the agent may do about a missing tool.

Decision (epic `qpshf`): this lives in the **role templates**, per ADR 0038 — copied into new projects, never resolved live. No launcher preamble.

## Documents
- `SPEC.md`, "Role profile templates" — reproduces the prompts verbatim; a test compares it with the files, so both change together.
- `ARCHITECTURE.md`, "Session container specification", the "Root filesystem" row: "agents install tools into it" becomes precise — user-level installs only, because the process is uid 1000 with `CapDrop: ALL` and `no-new-privileges`; system packages belong in the image.
- ADR 0038, "Consequences" — no change to the decision; nothing to edit unless the wording about what templates contain needs it.

## Acceptance criteria
- [ ] `orchestrator/src/projects/templates/implementer.md`, `reviewer.md` and `merger.md` gain the paragraph; `planner.md` does not (it builds nothing). One shared wording, adjusted only where the role needs it.
- [ ] The paragraph says, in prose and briefly (it is paid for in every session's context):
  - the session runs in a disposable container that belongs to this session alone; installing into it is expected and needs no permission;
  - a missing compiler, toolchain version, linter or test runner is installed, not reported as a blocker — including the toolchain version the repository pins;
  - there is no root: no `apt`, no `sudo`. Installs are user-level (examples: `rustup`, `cargo install`/`cargo binstall`, `npm install -g` or `npx`, a Python venv);
  - what needs root or a system library cannot be fixed from inside; say so in the task comment / hand-off instead of working around it;
  - installs are not part of the work: nothing installed is committed, and the repository is not changed to suit the container.
- [ ] The text names no image and no tool as *present* — a profile's image is editable, so the paragraph must hold on any image honouring the session image contract. Tools appear only as examples of how to install.
- [ ] `SPEC.md`, "Role profile templates" carries the new text verbatim and the template-vs-SPEC test passes; `tests/profile_templates.rs` and `tests/profiles.rs` expectations updated if they assert prompt content or length.
- [ ] Prompts stay under `MAX_SYSTEM_PROMPT_BYTES`.
- [ ] Backend quality chain passes.

## Implementation notes
- Reaches new projects only (ADR 0038). Existing projects: recreate the profile from the template (`New profile` → `Start from`), or paste the paragraph. Put that one sentence in the PR/commit body, not in the docs — ADR 0038 already states the rule.
- Independent of the image tasks: the statement is true on today's base image as well (rustup into `$HOME` works there already), so this can land first and is the quickest relief for the reported problem.

## Edge cases
- `$HOME` (`/session/home`) persists for the session's life and across a resume, but a relaunch gets a new container: installs outside `$HOME` and `/session/work` are gone. Worth half a sentence so the agent re-installs without surprise instead of concluding the environment is broken.