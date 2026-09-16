# 0019. Defer isolation of git operations on agent-controlled checkouts

Status: accepted. Supersedes ADR 0012's unconditional containment claim for this known vulnerability; the intended container boundary remains the design goal.

## Context

The orchestrator runs git against session checkouts whose files and local git configuration are writable by the agent. Git can execute repository-configured helper commands, for example a `core.fsmonitor` hook during a status/index operation. A harmless local probe during the documentation review confirmed that this configuration can execute a command in the process environment running git.

If the orchestrator performs such an operation on an agent-modified checkout, agent-controlled code could execute with the orchestrator's privileges. Those privileges include access to application secrets, project data and the container engine socket. Reference clones, argument-vector subprocess calls and the absence of an engine socket inside session containers do not by themselves close this path. Trusted human users do not eliminate the risk of an agent making unsafe changes or following malicious repository content.

## Decision

Accept this as a known, unresolved v1 vulnerability and defer remediation until after v1. Keep the existing orchestrator-side git design. Do not add a restricted git helper container as a v1 requirement; its implementation and deployment complexity are deliberately deferred.

Options considered:

1. Isolate operations on agent-controlled checkouts in a restricted helper container, without application secrets or an engine socket, and keep credentialed upstream operations against trusted repositories. Deferred because of the added v1 complexity.
2. Rely on disabling selected git hooks or configuration settings. Not accepted as a complete fix: all execution-capable configuration and repository-controlled paths would need to be considered and verified.
3. Document and accept the current exposure for v1. Chosen explicitly during review.

## Consequences

- The container is an intended permission boundary, but v1 does not guarantee containment of malicious agent-controlled git metadata. Documentation must state this exception instead of claiming that an agent can affect only its own container.
- Post-v1 remediation must prevent agent-controlled git metadata and helpers from executing with orchestrator privileges. A restricted helper is the current candidate, not a committed implementation choice.
- Remediation verification must include the fsmonitor reproduction and other supported execution-capable git configuration, proving that they cannot access the orchestrator's secrets, engine socket or unrelated project data.
- This is an accepted risk, not an open design question or a fixed vulnerability. It remains documented until remediation is implemented and verified.
