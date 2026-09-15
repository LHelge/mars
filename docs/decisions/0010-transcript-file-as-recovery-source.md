# 0010. Transcript file, not the attach stream, is the recovery source

Status: accepted

## Context

The orchestrator reads the CLI's stdout to produce events. If it only reads the container attach stream, anything emitted while the orchestrator was down or disconnected is lost, and the engine's own log buffer is not a reliable substitute (rotation, size limits, engine-specific drivers).

## Decision

The session container's entrypoint runs the CLI with stdout piped through `tee -a /session/log/stream.jsonl` on the session volume. The orchestrator tails that file (by byte offset) as its input, whether or not it is also attached to the container. After a restart it lists containers by the `mars.session_id` label, reattaches for stdin, and resumes tailing each file from the offset recorded with the last committed event. The attach stream is only used for stdin and for liveness.

Each `events` row records the byte offset of the end of the native line it came from, in the payload's `_offset` field, so recovery has an exact restart point.

## Consequences

- No event is lost across orchestrator restarts or reconnects; at worst one is duplicated, and `seq` derivation plus offset checks prevent that.
- Disk usage on the session volume grows with the transcript; large tool outputs are the main cost. Rotation is not needed for v1 because parked sessions are cheap and the file is bounded by the session's life.
- The entrypoint script is part of the session image contract (`ARCHITECTURE.md`, "Session image").
