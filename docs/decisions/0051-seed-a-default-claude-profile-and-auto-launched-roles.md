# 0051. Seed a default `claude` profile, and seed the implementer and reviewer ephemeral and auto-launched

Status: accepted (Bears epic `xz6yq`). Supersedes ADR 0038 in one respect: a seeded profile may now spend money without a person launching it, though only on work somebody queued.

## Context

A new project's default profile was the `implementer`, so the launch form offered a role even to a person who only wanted a Claude Code session. And with the dispatcher (ADR 0042) and automatic merges (ADR 0045), a board still needed a person to launch the implementer and the reviewer for every task.

## Decision

Creation seeds a fourth profile, `claude`, first and as the default: conversational, serving no state, no git tools, a short prompt naming the task tools. `implementer` and `reviewer` are seeded `ephemeral` with `auto_launch`, so a task in `ready` travels to `done` unattended. `auto_launch` is seeded unconditionally: the dispatcher already skips a profile whose credential does not resolve (`no_credential`), so the roles start once a `global` or `project` credential is stored. Existing projects are not migrated (ADR 0038).

Rejected: **a "no profile" launch**, a nullable `sessions.profile_id` with defaults in code, which touches the launcher, recovery and MCP auth only to model an absence.

Rejected: **seeding `auto_launch` only when a global credential exists at creation**, which a credential stored later would never switch on; and **relaxing the save-time credential rule** for every profile (ADR 0036, 0042).

Rejected: **`claude` serving `ready`**, or every queue state: it would see the implementer's queue.

## Consequences

- Saving a seeded implementer or reviewer without such a credential is the ordinary 400 of the auto-launch toggle.
- The dispatcher asks for the credential only once a profile has a claimable task, so an idle new project logs nothing.
- Both role prompts say that a run with no conversation asks through `needs_human`.
