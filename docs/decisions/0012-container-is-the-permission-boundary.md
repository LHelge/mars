# 0012. The container is the permission boundary

Status: accepted; the mount list in the decision is extended by 0015 (per-project CLI state directory and shared directories).

Superseded by [0019](0019-defer-isolation-of-git-checkout-operations.md) for the unconditional containment claim: v1 accepts a known path for agent-controlled git configuration to execute with orchestrator privileges. The intended boundary remains the design goal.

## Context

Coding agents are most useful with full auto-permissions, and interactive prompts do not work for unattended sessions anyway. Fine-grained tool allow-lists inside the CLI are fragile and bypassable by any shell command. The alternative is to accept that the agent can do anything inside a boundary and make the boundary strong.

## Decision

Users are authenticated and trusted; agents run with bypass-permissions mode inside a container that is the only thing they can affect:

- one container per session, created and removed by the orchestrator, never shared;
- no engine socket, an unprivileged user, an internal network, and only the session volume (RW) and project repository (RO) mounted;
- only profile-declared secrets injected, never orchestrator-only ones;
- every effect outside the container (git push, task changes) goes through MCP tools with a per-session bearer token and is attributed and audited.

Egress restriction and a sandboxed runtime are later hardening steps on the same boundary, selectable per profile.

## Consequences

- A compromised or misbehaving agent can destroy its own working copy and burn its API budget, nothing more (subject to ADR 0019).
- The orchestrator container is the high-value target and is kept minimal: unprivileged user, no shell tools beyond `git`, read-only root filesystem where the engine allows it.
- Model credentials injected into a session are visible to that agent; prefer per-user OAuth tokens over shared keys when cost attribution matters.
