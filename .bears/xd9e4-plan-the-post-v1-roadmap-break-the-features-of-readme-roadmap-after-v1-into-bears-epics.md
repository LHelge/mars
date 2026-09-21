---
id: xd9e4
title: "Plan the post-v1 roadmap: break the features of README \"Roadmap after v1\" into Bears epics"
status: open
priority: P3
created: "2026-09-21T10:03:35.790870241Z"
updated: "2026-09-21T10:03:35.790870241Z"
tags:
  - planning
  - roadmap
---

## Summary
A reminder to plan, not to build. `README.md`, "Roadmap after v1" lists the features deferred from v1 as one paragraph; nothing in Bears tracks them. Go through the list with the user, decide what is next and in what order, and turn each chosen feature into an epic with tasks that cite the document sections they implement.

## The list today (`README.md`, "Roadmap after v1")
- A dispatcher that launches ephemeral sessions when a served task state has claimable work, bounded per profile.
- Scheduled agents: a profile run on a cron expression (a daily tech-debt scan that files tasks; an agent that turns GitHub issues into backlog tasks).
- GitHub App credentials and webhooks.
- Egress restriction for session containers (`ARCHITECTURE.md`, "Networks": an allow-list on `mars-egress`).
- Sandboxed runtimes (gVisor, Kata) per profile.
- Per-project toolchain setup scripts for session images (`ARCHITECTURE.md`, "Session image": a setup script run by the entrypoint before the CLI). Related: epic `qpshf` adds the dev image and leaves this out of scope.
- A second agent backend — GitHub Copilot CLI is the candidate, pending a spike on its structured output, stdin protocol and container authentication.

## Acceptance criteria
- [ ] Re-read the README paragraph and `docs/open-questions.md` first; the list above is a snapshot from 2026-09-21 and may have moved.
- [ ] Each feature is either planned as an epic (`type: epic`, tasks under it with `parent`, ordered with `add_dependency`), or explicitly left for later with a one-line reason in this task's closing comment.
- [ ] Priorities and the order between epics are agreed with the user, not assumed.
- [ ] Features needing a decision before they can be broken down (the second backend's spike, the dispatcher's bounding rule) get a spike or ADR task as the epic's first task.
- [ ] No code and no document changes beyond what planning itself requires; the README paragraph changes when a feature ships, not when it is planned.