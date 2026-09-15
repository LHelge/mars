# 0012. The container is the permission boundary

Status: accepted

## Context

Coding agents are most useful with full auto-permissions: no prompts for edits, shell commands or network calls. Interactive permission prompts do not work for unattended sessions anyway. Fine-grained tool allow-lists inside the CLI are fragile and bypassable by any shell command. The alternative is to accept that the agent can do anything inside a boundary and make the boundary strong.

## Decision

Users are authenticated and trusted; agents run with the backend's bypass-permissions mode inside a container that is the only thing they can affect. Concretely:

- one container per session, created and removed by the orchestrator; sessions never share a container;
- session containers never receive the engine socket, run as an unprivileged user, on an internal network, with only the session volume (RW) and the project mirror (RO) mounted;
- only secrets the profile declares are injected, never orchestrator-only ones;
- every effect outside the container (git push, task changes) goes through the orchestrator's MCP tools with a per-session bearer token and is attributed and audited.

Egress restriction and a sandboxed runtime (gVisor/Kata) are hardening steps on the same boundary, selectable later per profile.

## Consequences

- A compromised or misbehaving agent can destroy its own working copy and burn its API budget, nothing more.
- The orchestrator container is the high-value target and is kept minimal: unprivileged user, no shell tools beyond `git`, read-only root filesystem where the engine allows it.
- Model credentials injected into a session are visible to that agent; per-user OAuth tokens should be preferred over shared keys when cost attribution matters.
