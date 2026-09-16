# 0020. Accept input-delivery uncertainty across orchestrator restarts in v1

Status: accepted. Qualifies restart durability for incoming messages; transcript-file recovery of agent output from ADR 0010 remains in scope.

## Context

Incoming messages wait in the in-memory registry while a session starts or resumes. The owner records a `user_message` event before writing to CLI stdin but does not track delivery durably. A restart can lose queued input, leave a transcript entry for input the agent never received, or leave delivery uncertain. Retrying uncertain input may make the agent do the work twice.

Options considered:

1. A durable input queue with delivery states, deduplication and UI handling for ambiguous delivery. Deferred for initial complexity; a durable queue alone still cannot prove whether the CLI consumed input immediately before a crash.
2. Keep the in-memory path and document the restart limitation. Chosen explicitly during review.

## Decision

Accept message loss and uncertain or repeated processing around orchestrator restarts as known v1 behavior. Keep the record-before-write input path; add no durable queue, delivery-status schema or restart-safe deduplication.

After a restart, v1 does not resend pending inputs. The operator inspects the conversation and decides whether to resend. `client_id` serves optimistic UI reconciliation, not durable idempotency.

This accepts disruption during orchestrator restart only; loss during ordinary browser disconnection or normal operation is not intended behavior.

## Consequences

- A message accepted just before restart may not reach the agent; a visible transcript entry does not prove delivery, and a missing response does not prove no work was done.
- Operators accept manual inspection and possible resubmission, which may repeat work.
- Durability claims distinguish recorded agent output from delivery of incoming messages.
- This limitation stays documented until durable delivery is implemented and verified.
