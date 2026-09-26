---
id: d6vuy
title: "git_session: alternates assertion fails on macOS, where /var resolves to /private/var and git 2.55 writes both paths"
status: done
priority: P3
created: "2026-09-25T10:08:20.641391Z"
updated: "2026-09-26T18:59:43.981131085Z"
tags:
  - orchestrator
  - tests
  - git
attempts: 1
---

Found while verifying epic `xz6yq` on macOS (git 2.55.0, Podman). Four tests in `orchestrator/tests/git_session.rs` fail on `main`, independent of that epic's change: `an_omitted_base_starts_from_the_project_default_branch`, `a_fully_qualified_hand_off_ref_is_a_usable_base`, `every_supported_base_kind_resolves_and_produces_a_clone_at_that_commit`, `a_relaunch_replaces_whatever_a_failed_attempt_left_behind`.

The assertion at `tests/git_session.rs:163` compares the clone's `.git/objects/info/alternates` exactly against one line. On this host git writes the mirror's objects path twice: once canonicalised (`/private/var/folders/.../repo.git/objects`) and once as given (`/var/folders/.../repo.git/objects`), because the macOS tempdir `/var` is a symlink to `/private/var`.

Decide whether production cares (`ARCHITECTURE.md`, "Git model" → "Session clone"): does a duplicated alternates line matter for the session container, where the mirror is mounted at a fixed path? Then either canonicalise the paths the clone is given (`src/git/`), or make the test compare the set of canonicalised lines. Linux CI does not see it.