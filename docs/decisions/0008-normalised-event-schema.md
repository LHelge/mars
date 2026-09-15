# 0008. Normalised `AgentEvent` schema

Status: accepted

## Context

Claude Code emits `stream-json` lines that change between versions, and any second backend will emit something else again. The frontend could render the native formats directly, with a renderer per backend, or the backend could translate everything into one schema.

Passing native JSON through is quicker to start and loses nothing. But it couples every frontend component to every CLI's format and version, doubles the rendering code when the second backend lands, and makes the stored history depend on formats we do not control.

## Decision

The orchestrator translates each backend's native output into one `AgentEvent` schema (`SPEC.md`, "AgentEvent") before storing it. The frontend only ever sees `AgentEvent`. Each backend adapter owns its translation, and unknown native messages become `AgentEvent::Raw` with the original payload so nothing is dropped. The native line is also kept verbatim in the transcript file on the session volume.

## Consequences

- One transcript renderer, one reducer, one storage format.
- New CLI message types show up as `Raw` until the adapter learns them; the UI renders `Raw` as a collapsed JSON block instead of breaking.
- The schema is versioned by adding optional fields only; a breaking change is a new kind.
- Translation is a pure function `native line -> Vec<AgentEvent>` and is unit-tested against recorded fixtures per CLI version.
