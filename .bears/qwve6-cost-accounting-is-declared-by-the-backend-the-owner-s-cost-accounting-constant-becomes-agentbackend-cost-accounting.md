---
id: qwve6
title: "Cost accounting is declared by the backend: the owner's COST_ACCOUNTING constant becomes AgentBackend::cost_accounting()"
status: open
priority: P2
created: "2026-09-21T12:17:34.121800Z"
updated: "2026-09-21T12:17:34.121800Z"
tags:
  - orchestrator
  - agent
  - sessions
parent: fgbm3
---

## Summary
`session/owner.rs` already has a `CostAccounting` enum with a per-turn and a `Cumulative` rule, selected by the constant `COST_ACCOUNTING = Cumulative` because that is what Claude Code 2.1.274 reports. It is a property of the CLI, not of Mars: a backend that reports a turn's own cost would be charged wrongly. Move the choice to the backend. The token counters are read from `result.usage` by the keys `input_tokens` and `output_tokens`; state that as the contract a translator must meet.

## Documents
- `ARCHITECTURE.md`, "Cost accounting": the increase-over-previous rule becomes "for a backend that declares its totals cumulative (Claude Code)"; add the `usage` key contract. `SPEC.md`, "AgentEvent", `result`: `usage` carries at least `input_tokens` and `output_tokens` when the backend knows them, whatever else it carries.

## Acceptance criteria
- [ ] `AgentBackend::cost_accounting() -> CostAccounting`; Claude answers `Cumulative`; the constant is gone; the enum moves to `agent/` (the owner imports it).
- [ ] The owner's baseline memory (`last_result_cost`) and the recovery replay (~l.1771) use the backend's rule; Claude's session counters are identical to today's on the existing tests.
- [ ] A `result` with `cost_usd: None` adds tokens and no cost under either rule (already true — assert it).
- [ ] `MockAgentBackend` can declare either rule; one owner test per rule.

## Implementation notes
- `session/owner.rs` ~l.118–138, `cost_delta` ~l.867, `charge_cost`; `agent/mod.rs`, `agent/mock.rs`.
- The stored event keeps the backend's own `cost_usd` value untouched (ADR 0008: additive schema only); only what is *added to the session counters* depends on the rule.

## Testing
- The existing `charge_cost` unit tests stay; owner tests through the mock backend.