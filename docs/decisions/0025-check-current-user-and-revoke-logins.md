# 0025. Check current user authority and revoke logins on password changes

Status: accepted.

## Context

JWT claims carried administrator status and the password-change flag without specifying how existing logins respond to account changes. Password changes, resets, deletion and demotion need predictable effects on refresh tokens and open browser connections.

Options considered:

1. Trust token snapshots until expiry. Rejected: demoted administrators keep privileges and password changes do not invalidate logins.
2. A blacklist of access tokens with revocation broadcast to every connection. Rejected for v1: needless token storage and connection coordination.
3. Check the current user and a per-user login version in Postgres. Chosen: one version field plus existing transactions give the required behavior.

## Decision

- `users.auth_version` is carried in access-token claims. Authenticated requests require a current user with the same version; authorization uses the database's current role and flags, never stale claims.
- Every password change or reset increments the version, revokes refresh tokens and invalidates reset links in one transaction. Self-service change issues a replacement pair for the current browser; administrator changes and reset links do not log the target user in.
- Credential issuance locks the user row and revalidates after locking, so a concurrent login or refresh cannot outlive a password change.
- Open streams recheck user state and version at heartbeat ticks and before incoming WebSocket messages. Invalid authorization closes the connection; the agent session continues.

## Consequences

- One database lookup per request and per stream check; no blacklist, extra service or broadcast.
- Passive streams may stay open until the next heartbeat. Already authorized work may finish; this is not cancellation.
- Login revocation does not revoke session MCP credentials or stop agents.
