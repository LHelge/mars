---
id: dbvwe
title: "PUT /projects/{id}: a default_branch move must not outlive a failed row write"
status: open
priority: P3
created: "2026-09-18T11:09:46.766794158Z"
updated: "2026-09-18T11:09:46.766794158Z"
tags:
  - orchestrator
  - projects
  - git
depends_on:
  - cgj5v
parent: pkaee
---

## Summary
Found while reviewing `cgj5v`. `routes/projects.rs::update` moves the bare repository's symbolic `HEAD` (`git::mirror::set_default_branch`, under the project git lock) and only then writes the row. When the same request also fails the row write — a `name` that collides (409 "project name already taken"), or the project being deleted in between (404) — `repo.git`'s `HEAD` names the new branch while `projects.default_branch` still names the old one.

## Documents
- `SPEC.md` "Projects" (`PUT /projects/{id}`: "checks the name against the project repository and moves that repository's `HEAD` to it in the same request").
- `ARCHITECTURE.md` "Git model" → "Project clone" (bare `HEAD` points at the default integration branch), "Serialization" (git lock before any database lock).

## Acceptance criteria
- [ ] A `PUT` carrying both a colliding `name` and a new `default_branch` on a `ready` project answers 409 and leaves `git symbolic-ref HEAD` in `repo.git` unchanged.
- [ ] Either order the work so nothing irreversible happens before the row write can no longer fail (hold the git lock, verify the head exists, write the row in a transaction, move `HEAD`, commit; on a `HEAD` failure roll back), or restore the previous `HEAD` when the row write fails. The git lock is still taken before any database lock and no transaction is open while waiting for it.
- [ ] Integration test in `tests/projects.rs` for the 409 case asserting `HEAD` through the existing `head_branch` helper.

## Documentation
- none expected; if the chosen ordering changes observable behaviour, `SPEC.md` "Projects" in the same commit.