---
id: csn2w
title: Port midgaard's eval harness to Curiosity so a model or prompt change is measured, not guessed
status: open
priority: P3
created: "2026-09-21T12:23:18.345804Z"
updated: "2026-09-21T12:23:18.345804Z"
tags:
  - curiosity
  - backlog
  - evals
depends_on:
  - bywt4
---

## Summary
`../midgaard/eval/` (`prep-arm.sh`, `run-arm.sh`, `run-case.sh`, `receipts.sh`, `summarize.sh`, `reviewer/`) is how the agent was tuned against OpenRouter with GLM-5.3-flash. Owning an agent means owning its quality; bring the harness over, driving `curiosity run`.

## Acceptance criteria
- [ ] `curiosity/eval/` with the scripts adapted to `curiosity run` and its JSON-line events; cases that depended on Bears, roles or worktrees are rewritten as plain repository tasks or dropped, each drop noted.
- [ ] A baseline run with the default model is recorded (pass rate, tokens, cost) in `curiosity/eval/README.md`; no credentials or provider responses containing them are committed (rule 3).
- [ ] Never run in CI; documented as a manual, credentialed step.

## Testing
- The harness runs end to end on at least one case against the mock provider, so the scripts themselves are exercised without a key.