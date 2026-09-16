# 0019. Defer isolation of git operations on agent-controlled checkouts

Status: accepted. Supersedes ADR 0012's unconditional containment claim for this known vulnerability; the intended container boundary remains the design goal.

## Context

The orchestrator runs git against session checkouts whose files and local git configuration the agent can write. Git executes repository-configured helpers, for example a `core.fsmonitor` command during a status or index operation; a harmless local probe during the documentation review confirmed it. Agent-controlled code could therefore run with the orchestrator's privileges: application secrets, project data and the engine socket. Reference clones, argument-vector subprocess calls and the absence of an engine socket in session containers do not close this path.

Options considered:

1. Run operations on agent-controlled checkouts in a restricted helper container without secrets or an engine socket. Deferred for v1 complexity.
2. Disable selected hooks or configuration settings. Not accepted as complete: every execution-capable setting and repository-controlled path would need to be found and verified.
3. Document and accept the exposure for v1. Chosen explicitly during review.

## Decision

Accept this as a known, unresolved v1 vulnerability and defer remediation until after v1. Keep the orchestrator-side git design. A restricted git helper container is not a v1 requirement.

## Consequences

- v1 does not guarantee containment of malicious agent-controlled git metadata. Documentation states this exception rather than claiming an agent can affect only its own container.
- Post-v1 remediation must stop agent-controlled git metadata and helpers from executing with orchestrator privileges; a restricted helper is the candidate, not a committed choice. Its verification must include the fsmonitor reproduction and other execution-capable git configuration.
- This is an accepted risk, not an open question or a fixed bug. It stays documented until remediation is implemented and verified.
