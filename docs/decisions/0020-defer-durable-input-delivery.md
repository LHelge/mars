# 0020. Accept input-delivery uncertainty across orchestrator restarts in v1

Status: accepted. Qualifies restart durability for incoming messages; transcript-file recovery of agent output from ADR 0010 remains in scope.

## Context

Incoming session messages can wait in the in-memory registry while a session starts or resumes. The owner records a `user_message` event before writing to CLI stdin, but does not durably track delivery. A restart can therefore lose queued input, leave a transcript entry for input the agent never received, or leave delivery uncertain when a crash occurs around the stdin write. Retrying an input whose delivery is uncertain may cause the agent to perform the work twice. An accepted response and the presence of a transcript entry are not proof of agent receipt or execution.

Options considered:

1. Add a durable input queue, delivery states, request deduplication and recovery/UI handling for ambiguous delivery. Deferred because of the added initial complexity; a durable queue alone would still not prove whether the CLI consumed input immediately before a crash.
2. Keep the in-memory input path and document the restart limitation. Chosen explicitly during review.

## Decision

Accept message loss and uncertain or repeated processing around orchestrator restarts as known v1 behavior. Keep registry queues and the existing record-before-write input path. Do not add a durable input queue, delivery-status schema/UI, or restart-safe input deduplication as a v1 requirement.

After a restart, v1 does not automatically reconstruct and resend pending inputs from `user_message` events or client reconnection state. The operator can inspect the conversation and decide whether to resend. `client_id` continues to serve optimistic UI reconciliation, not a durable idempotency guarantee.

Existing container adoption, transcript tailing and replay of stored output events remain in scope. This decision accepts disruption during orchestrator restart; it does not make loss during ordinary browser disconnection or normal operation an intended behavior.

## Consequences

- A message accepted immediately before restart may not reach the agent. A message visible in the transcript may not have been delivered, and a missing response does not prove that the agent did no work.
- Operators accept manual inspection and possible resubmission after a restart; resubmission may repeat work.
- Durability claims must distinguish recorded agent output from delivery of incoming messages. No new delivery indicator is required in v1.
- Durable input delivery, deduplication and handling of ambiguous delivery are deferred until after v1. This limitation remains documented until that behavior is implemented and verified.
