---
id: ahv3c
title: Drop a session's ref at session end when a hand-off ref or an integration head already holds its tip
status: done
priority: P2
created: "2026-09-24T08:17:13.290161Z"
updated: "2026-09-24T12:33:34.684308157Z"
tags:
  - orchestrator
  - frontend
  - git
  - docs
depends_on:
  - dhzqz
parent: kc8k3
attempts: 1
---

Part of epic kc8k3; read its decision first, and the ADR written by dhzqz. dhzqz drops the ref of a session that made no commits. This task drops the ref of a session whose commits are all kept elsewhere: an implementer that handed off its last commit, a reviewer that started from a hand-off and committed nothing new, or a session whose branch was merged into an integration head.

## Scope

1. **End-of-session rule.** In the same end-of-session fetch-back as dhzqz, under the project git lock: after fetching, if the session's tip is an ancestor of (or equal to) some `refs/handoffs/*` of the project or some integration head (`refs/heads/*`), delete `refs/sessions/<sid>`. Upstream-tracking refs and tags do not count: Mars does not own them, and a fetch can prune them (ADR 0017). Use one `git for-each-ref --contains <tip>` (or equivalent) over those two namespaces, not a loop of `merge-base` calls. Mid-session syncs are unchanged.
2. **A session with work of its own keeps its ref.** Commits after the last hand-off, or work never handed off, mean the tip is contained in nothing retained, so the ref stays until the session is deleted (g3qdk, including its unmerged-commits warning).
3. **Later removals are not chased.** Hand-off refs go only with their task or project, and integration heads only move forward, so a ref judged redundant at the end stays redundant. A ref kept at the end because it held unique work is not re-examined later (for example after that work was merged). Decide whether the hourly orphan cleanup job should apply this rule to the refs of ended sessions too, and document the choice. The default is not to, to keep the job to orphans.
4. **Readers.** The same readers dhzqz made safe for a missing ref must also give a useful answer here. In particular the "Changes" panel of an ended session whose ref was dropped should still show its work. Choose and document how: fall back to the session's last hand-off revision (`handoff_id` of revisions the session published), or state plainly that the work lives in the hand-off, with a link to it.

## Documentation (rule 1)

- ARCHITECTURE.md: "Git model", Ref ownership and the end-of-session fetch-back, extended by the containment rule; the "Background jobs" orphan cleanup row if item 3 extends it.
- SPEC.md: whatever the readers now answer, and the Frontend wording of the Changes panel for such a session.
- Amend the ADR from dhzqz only if a detail changed from what it records; otherwise it already covers this.

## Tests

- Integration (real bare repositories): an implementer that handed off its tip and ended loses its ref, and one that committed past its last hand-off keeps it; a reviewer launched from a hand-off that committed nothing loses its ref; a session merged into `main` before ending loses its ref; a tip contained only in `origin/main` or a tag keeps its ref.
- The Changes panel answer for an ended session whose ref was dropped, and a Playwright scenario for it with its row in `frontend/tests/README.md`.