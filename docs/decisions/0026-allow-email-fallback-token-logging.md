# 0026. Intentionally log invitation and reset links when email is unconfigured

Status: accepted. Clarifies the logging exception for the fallback already specified in ADR 0014.

## Context

ADR 0014 requires `LogEmailClient` to log full messages when no email API key is configured, but the agent instructions prohibit logging any secret or token. Logging usable invitation and password-reset links is wanted behavior: it makes local development possible without configuring email.

## Decision

Keep automatic `LogEmailClient` selection when `RESEND_API_KEY` is unset. Log the complete invitation and password-reset messages, including usable token-bearing links, at `info`. Do not redact these links or add another opt-in flag.

Treat this delivery path as an explicit exception to the secret-logging rule. Other credentials remain excluded. Configured email delivery and ordinary request/error logs do not log the links; a provider failure does not activate the fallback.

## Consequences

Developers can copy links from local orchestrator logs and complete invitation/reset flows without email setup. These operator-accessible logs intentionally contain the delivery tokens. Token hashing in the database, expiry, single-use checks and revocation remain unchanged. The `Invite` API response still excludes the token.
