# 0014. Email through Resend behind an `EmailClient` trait

Status: accepted

## Context

Invites and password resets need outbound email. Options: SMTP through a generic client library, which every provider supports; a specific provider's HTTP API; or no email at all with admins handing out temporary passwords.

SMTP is universal but brings its own configuration surface (ports, STARTTLS, credentials, sender verification) and a heavier dependency. A provider API is one key and one HTTP call.

## Decision

Email is sent through the Resend HTTP API using `reqwest`, behind an `EmailClient` trait with three implementations: `ResendClient` (production), `LogEmailClient` (used automatically when `RESEND_API_KEY` is unset; writes the full message including the link to the orchestrator log at `info`), and `MockEmailClient` (tests; captures messages for assertions). Templates are plain Rust string formatting for v1; there are two messages (invite, password reset).

## Consequences

- One environment variable (`RESEND_API_KEY`) plus `MAIL_FROM` configures delivery; the sender domain must be verified in Resend.
- A deployment without email still works: the admin copies invite links out of the log.
- Switching provider later is one new `EmailClient` implementation; SMTP can be added the same way if a deployment needs it.
- Tests never touch the network; the mock is the only client under `integration-tests`.
