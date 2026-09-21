---
id: xd9e4
title: "Plan the post-v1 roadmap: break the features of README \"Roadmap after v1\" into Bears epics"
status: done
priority: P3
created: "2026-09-21T10:03:35.790870241Z"
updated: "2026-09-21T20:26:30.705900886Z"
tags:
  - planning
  - roadmap
attempts: 1
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
- [x] Re-read the README paragraph and `docs/open-questions.md` first; the list above is a snapshot from 2026-09-21 and may have moved.
- [x] Each feature is either planned as an epic (`type: epic`, tasks under it with `parent`, ordered with `add_dependency`), or explicitly left for later with a one-line reason in this task's closing comment.
- [x] Priorities and the order between epics are agreed with the user, not assumed.
- [x] Features needing a decision before they can be broken down (the second backend's spike, the dispatcher's bounding rule) get a spike or ADR task as the epic's first task.
- [x] No code and no document changes beyond what planning itself requires; the README paragraph changes when a feature ships, not when it is planned.
## Closing comment (2026-09-21)
Planned with the user. `docs/open-questions.md` has nothing open; the README paragraph had gained two items since the snapshot above (git-execution isolation, ADR 0019; durable input delivery, ADR 0020).

**v2 — automation (planned now, both P1, above ACP/Curiosity):**
- Dispatcher → epic `qabvt` (8 tasks; first are the ADR write-up `jrgm7` and the launch-path extraction `rgrvp`).
- Scheduled agents → epic `tup8z` (6 tasks; depends on `qabvt`'s extraction, schema and capacity check). The GitHub-issues instance is excluded: it needs v3.
- The rules both ADR tasks write down were settled in this session and are recorded in the two epics' "Decisions" sections, so those tasks are write-ups, not open questions.

**Already planned elsewhere:**
- Second agent backend → the ACP/Curiosity epics `fgbm3`, `w9nsq`, `vj82v`, `eydgf`, `ddb8s`, first task the spike `j23cg`. The README still names Copilot CLI as the candidate; `j23cg`'s ADR is what changes that. Demoted with their tasks from P1 to P2: automation is worth more, even with Claude Code as the only backend.

**Left for later:**
- v3 — GitHub integration (GitHub App credentials, webhooks, the issues-to-backlog scheduled agent): after automation, which is what gives it something to trigger.
- v4 — isolation and hardening (git-execution isolation per ADR 0019, egress restriction on `mars-egress`, sandboxed runtimes per profile, durable input delivery per ADR 0020, per-project toolchain setup scripts): not automation and not GitHub; revisit together.

Each of v3 and v4 needs a planning task of its own when its turn comes; none is filed yet.
