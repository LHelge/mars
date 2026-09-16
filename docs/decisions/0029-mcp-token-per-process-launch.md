# 0029. Generate a fresh MCP token for each session process launch

Status: accepted.

## Context

The MCP configuration contract says the original bearer token can be regenerated from its stored hash. A hash supports verification, not reconstruction. The lifecycle needs to distinguish starting a new process from reconnecting to one that is already running.

## Decision

Generate a fresh cryptographically random MCP token for every actual process launch, including resume and conversational retry. Persist its SHA-256 hash and write matching `mcp.json` before starting the container. On initial creation the hash is part of the inserted session row; on relaunch it replaces the previous hash. The old process must no longer be running, and the existing per-session launch path serializes preparation.

The file is written through atomic replacement. If either database or file preparation fails, do not start the process; follow existing launch-failure handling. A subsequent launch attempt generates another token, without reconstructing the earlier one. No encrypted token column is added.

When the orchestrator adopts an already-running process after a restart, retain the existing hash and configuration. Browser reconnection and user-login changes do not rotate session MCP credentials.

## Consequences

- Each new process receives a token matching the persisted hash; subsequent requests with a replaced token fail authentication.
- Adoption does not invalidate credentials held by a live process.
- Database and file preparation are not atomic together, but process startup waits for both. Interrupted preparation needs no token-recovery mechanism.
- Acceptance covers first launch, parked-session resume, conversational retry, invalidation of replaced tokens, failure before configuration is ready, and adoption without token rotation.
