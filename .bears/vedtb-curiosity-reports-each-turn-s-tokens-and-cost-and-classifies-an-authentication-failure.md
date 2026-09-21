---
id: vedtb
title: Curiosity reports each turn's tokens and cost, and classifies an authentication failure
status: open
priority: P2
created: "2026-09-21T12:20:08.432555Z"
updated: "2026-09-21T12:20:08.432555Z"
tags:
  - curiosity
  - acp
  - usage
depends_on:
  - pfs5r
parent: vj82v
---

## Summary
Mars accumulates `cost_usd`, `input_tokens` and `output_tokens` per session from each turn's `result` (`ARCHITECTURE.md`, "Cost accounting") and parks a session with a fatal `error` naming the credential when the CLI cannot authenticate. ACP's usage reporting is recent and uneven across agents (spike notes); on Curiosity it can be exact.

## Documents
- `curiosity/README.md`: the usage fields, their semantics (**per turn, never cumulative**), and the error classification.

## Acceptance criteria
- [ ] The `session/prompt` response carries the turn's usage — summed over every model call of the turn: input, output, cached-read, cache-write and reasoning tokens where the provider reports them — in ACP's standard usage field if the pinned protocol version has one, and always in `_meta` under a documented `curiosity` key, so the Mars adapter has one stable place to read.
- [ ] Cost in USD when the provider reports it: OpenRouter returns the request cost in its usage accounting (verify against the live API and record the request parameter it needs); Ollama has none → absent, not zero.
- [ ] `usage_update`-style context notifications (context used / window) are emitted if the protocol version supports them; optional.
- [ ] Provider errors are classified: authentication (401/403 from the provider), rate limit, context overflow, other. An authentication failure ends the turn with a JSON-RPC error whose `data` carries `{"kind":"authentication","credential":"OPENROUTER_API_KEY"}` — the variable's **name**, never its value — and is not retried.
- [ ] Midgaard's `Usage` accounting differences between Anthropic-shaped and OpenAI-shaped usage stay correct for the providers that remain.

## Testing
- Unit tests for summation and classification with the mock provider; the environment-gated live test asserts a non-zero cost from OpenRouter.