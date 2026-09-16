# 0027. Preserve agent output without automatic secret redaction in v1

Status: accepted. Qualifies the blanket confidentiality claims in ADR 0006.

## Context

The documentation promises that secrets never appear in events or in plaintext database records. Agents can print injected environment variables or credentials from files, and users can paste secrets into messages. The existing transcript and event paths preserve that content, so the promise exceeds the design.

Options considered:

1. Add automatic secret detection and redaction across transcript, event and display paths. Deferred beyond v1; this adds filtering machinery and cannot be assumed from the existing secret store.
2. Narrow the promise to credential handling and explicitly retain arbitrary agent/user content. Chosen for v1.

## Decision

The orchestrator must not copy credentials from its credential-handling paths into operational logs or its own generated event metadata. The invitation/reset-link logging exception in ADR 0026 remains intentional.

Agent/tool output and user-provided transcript content may contain secrets. Preserve it through the existing storage and display paths without automatic secret detection or redaction in v1. Normal event translation, size limits and backend-provided redaction remain unchanged. The `thinking.redacted` field does not promise Mars-level secret filtering.

## Consequences

Transcript files, event payloads and their backups may contain plaintext credentials. Envelope encryption protects values in the managed secret store, not copies emitted into arbitrary output. There is no v1 guarantee that transcripts or events are secret-free, and no new filtering component is required.
