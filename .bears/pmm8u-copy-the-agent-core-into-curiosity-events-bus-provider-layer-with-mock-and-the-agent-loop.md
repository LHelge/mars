---
id: pmm8u
title: "Copy the agent core into curiosity: events, bus, provider layer with mock, and the agent loop"
status: open
priority: P1
created: "2026-09-21T12:18:21.375056Z"
updated: "2026-09-21T12:18:21.375056Z"
tags:
  - curiosity
  - core
depends_on:
  - jneu8
parent: w9nsq
---

## Summary
Bring over the part of midgaard that is an agent and nothing else: the typed event stream, the bus that carries it, the `ModelProvider` trait with its rig adapter and its scripted mock, and the step/run loop with retry and cancellation. Source: `../midgaard/src/` at commit `0684a87` (if the checkout is not beside the repository, clone `github.com/LHelge/midgaard`).

## Documents
- `curiosity/README.md`: a module map. `ARCHITECTURE.md` gets nothing yet — Curiosity's behaviour as a Mars backend is documented when it becomes one.

## Acceptance criteria
- [ ] Copied with their unit tests: `event.rs`, `bus.rs`, `model.rs`, `model/mock.rs`, `agent.rs`, and whatever of `test_support.rs` they use.
- [ ] Couplings cut, not stubbed: `bus.rs` loses `history` and `runtime`; `event.rs` loses the kinds that only the fleet, Heimdall, the TUI or Bears produce (task claims, leases, nudges, decisions, process reaping stays for the bash task). `SessionMode`, `EndReason` and friends are reduced to what a single driven agent can be.
- [ ] Providers: OpenRouter (`OPENROUTER_API_KEY`, optional `OPENROUTER_BASE_URL`) and Ollama (`OLLAMA_HOST`) are kept with their request parameters (reasoning effort, the context-compression opt-out, cache-affinity session key). The direct Anthropic provider is **removed** for now: `ANTHROPIC_API_KEY` is the Claude backend's credential in Mars (ADR 0036) and a second owner of that name is a design question filed in the backlog, not something to inherit by copy.
- [ ] Model spec stays `provider/model`; an unknown provider is a configuration error naming the supported ones.
- [ ] No `midgaard`, `Yggdrasil`, `Heimdall`, `Mimir`, `bea` or task id (`(uy74r)`-style) reference remains in code or comments; comments that explained a decision keep the explanation.
- [ ] `cargo add` for every dependency actually used (rig-core with the same feature set, reqwest rustls, tokio, futures, serde, thiserror, tracing, tokio-util); nothing TUI-, Bears-, browser- or template-related.

## Implementation notes
- The dependency edges were mapped: `agent → bus, event, model, tool`; `model → event`; `bus → event, history, runtime`; `event → bus`. `tool.rs` arrives in the next task — land a minimal `Tool`/`ToolSet` here if the loop does not compile without it, and let the next task replace it; or take `tool.rs` in this task and leave the concrete tools for the next. Choose the split that keeps both commits green.
- `tests/live_provider.rs` comes along, still gated on its environment variable so CI never calls a provider.

## Testing
- The copied unit tests pass; the loop is exercised end to end against the mock provider. The crate's quality chain passes.