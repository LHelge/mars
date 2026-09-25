---
id: qwvb4
title: "A resumed Claude process's first result re-charges the whole conversation: seed the cost baseline from the last committed result"
status: done
priority: P1
created: "2026-09-25T17:57:19.311392301Z"
updated: "2026-09-25T18:03:00.240402782Z"
tags:
  - orchestrator
  - sessions
  - bug
attempts: 1
---

Implements `ARCHITECTURE.md`, "Cost accounting" (the increase-over-previous rule).

**Bug.** Since the 2.1.282 pin (yf5zf), `total_cost_usd` and `modelUsage` carry across `--resume`: `tests/fixtures/claude/2.1.282/resume_prompt_first.jsonl` reports 0.0469 and the resumed `resume_prompt.jsonl` reports 0.0901 (= 0.0469 + 0.0432; `modelUsage` tokens 2/667 → 4/674). On 2.1.274 the resumed value reset (0.0413 → 0.0414). The owner starts every process with `last_result_cost: None` (`session/owner.rs`), so every relaunch of a parked session adds all earlier cost again: turn A, park, resume, turn B stores 2A + B.

**Fix.** A launch that resumes a CLI session seeds the owner's baseline from the session's last committed `result` event's `cost_usd` (a fresh launch keeps `None`). The adopted-replay path is unchanged. The documented rule changes: the "starts its counters again from zero" sentence in `ARCHITECTURE.md` becomes the observed 2.1.282 behaviour, and `2.1.282/NOTES.md` records the cross-resume observation.

**Tests.** Owner test over the 2.1.282 `resume_prompt_first` + `resume_prompt` pair: the session's `cost_usd` ends at 0.0901, not 0.137.

Stored sessions stay over-counted; no backfill.

Overlaps with qwve6 (COST_ACCOUNTING becomes a backend declaration).