# 0025. Check current user authority and revoke logins on password changes

Status: accepted.

## Context

JWT claims previously carried administrator status and the password-change flag without specifying how existing logins respond to account changes. Password changes, resets, deletion and demotion need predictable effects, including for refresh tokens and already-open browser connections.

Options considered:

1. Trust token snapshots until expiry. Rejected: old administrator privileges would remain usable after demotion, and password changes would not invalidate existing logins.
2. Maintain a blacklist of individual access tokens and broadcast revocation to every connection. Rejected for v1: unnecessary token storage and connection coordination.
3. Check the current user and a per-user login version in Postgres. Chosen: one version field and the existing user/token transactions provide the required behavior.

## Decision

- Add `users.auth_version`, initially zero, to access-token claims. Authenticated HTTP requests validate the token and require a current user with the same version. Missing users and version mismatches return 401. Authorization uses the current database role and password-change flag; stale token claims grant no administrator privileges.
- Every successful password change or reset increments the version, updates the password, clears the password-change flag, revokes existing refresh tokens and invalidates outstanding password-reset links in one transaction.
- A self-service password change requires the current password and issues a replacement pair for the current browser. An administrator changing another user's password and a reset-link operation do not log that target user in. Role changes preserve ordinary logins while changing permissions on subsequent requests.
- Credential issuance and password/reset-token mutations lock the user row before token rows and revalidate credentials after locking. Concurrent login/refresh cannot preserve access through credentials invalidated by a password change. Return new credentials only after commit.
- Open streams continue across ordinary token expiry. Recheck current user state and login version at existing heartbeat ticks, and before incoming WebSocket application messages, including terminal input. Invalid authorization closes the connection and its terminal attachment. The underlying agent session continues.
- Reconnection refreshes credentials. A refresh 401 clears browser authentication and returns to login; transient errors use normal retry. A successful self-service password change installs its replacement credentials and reconnects streams.

## Consequences

- Authentication performs a database lookup per request and per stream authorization check. No token blacklist, additional service or revocation broadcast is required.
- Passive streams may remain open until the next heartbeat: 30 seconds for WebSocket, 15 seconds for SSE. Commands are rechecked before acceptance. Work already authorized before revocation may finish; this is not cancellation of running operations.
- User login revocation does not revoke the separate session MCP credentials or stop running agents.
- Acceptance covers old access/refresh tokens after all three password-change paths, preservation of the self-service browser login, deleted users, stale administrator claims, concurrent refresh/login versus password changes, reset-link reuse, and revocation of idle streams and terminal input.
