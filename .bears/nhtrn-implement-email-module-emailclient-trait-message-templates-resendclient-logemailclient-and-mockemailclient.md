---
id: nhtrn
title: "Implement email module: EmailClient trait, message templates, ResendClient, LogEmailClient and MockEmailClient"
status: open
priority: P1
created: "2026-09-16T20:26:45.506118046Z"
updated: "2026-09-16T20:26:45.506118046Z"
tags:
  - orchestrator
  - auth
parent: qacxf
---

## Summary
Deliver `orchestrator/src/email/`: the `EmailClient` trait, the `EmailMessage` type with the three message templates (invitation, password reset, task escalation), the production `ResendClient` over `reqwest`, the `LogEmailClient` fallback that intentionally logs full token-bearing links at `info` when `RESEND_API_KEY` is unset, and the `MockEmailClient` behind the `integration-tests` feature. Startup selects the client from configuration. Every later auth flow and the tracker's escalation email send through this trait.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (module `email/`, `AppState` holds `Arc<dyn EmailClient>`, crate `reqwest`, `EmailError` variant of `Error`).
- `ARCHITECTURE.md` "Task tracker", paragraph "Notification" (escalation email content: project, task, reason, link).
- `SPEC.md` "Users (`/api/users`)", paragraph starting "When `RESEND_API_KEY` is unset".
- `README.md` "Configuration" rows `RESEND_API_KEY`, `MAIL_FROM`, `PUBLIC_URL`.
- `CLAUDE.md` rule 3 (secret logging exception).
- ADR 0014, ADR 0026.

## Acceptance criteria
- [ ] `EmailClient` is dyn-compatible (`Arc<dyn EmailClient>`), exposes `send` and `as_any()`; `EmailError` is `#[from]`-convertible into the prelude `Error` and maps to a 500 with a generic message (never the provider body).
- [ ] `EmailMessage::invitation(to, link, expires_at)`, `EmailMessage::password_reset(to, link, expires_at)` and `EmailMessage::escalation(to, project_name, task_number, task_title, reason, link)` build plain-text messages containing the respective link; escalation names the project, task number and title, the reason and the link.
- [ ] `ResendClient` POSTs `{ "from", "to": [..], "subject", "text" }` to `https://api.resend.com/emails` with `Authorization: Bearer <RESEND_API_KEY>`; a non-2xx answer or transport failure returns `EmailError`; it never logs the message text or the link at any level, and a provider failure does not fall back to logging.
- [ ] `LogEmailClient::send` writes one `tracing::info!` record that contains the recipient, subject and the complete message text including the usable link.
- [ ] `MockEmailClient` (feature `integration-tests`) captures every sent message (`sent() -> Vec<EmailMessage>`), supports `fail_next()` to simulate a provider failure, and implements `as_any()` for downcasting from `AppState`.
- [ ] Startup (`main.rs`) selects `ResendClient` when `RESEND_API_KEY` is set (and fails fast naming `MAIL_FROM` if that is missing), otherwise `LogEmailClient`, logging which one at `info`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/email/mod.rs` (trait, `EmailMessage`, `EmailError`, templates), `orchestrator/src/email/resend.rs`, `orchestrator/src/email/log.rs`, `orchestrator/src/email/mock.rs` (`#[cfg(feature = "integration-tests")]`). Wire the selection into `orchestrator/src/main.rs`; `AppState.email: Arc<dyn EmailClient>`.
- Trait shape (no `async_trait` crate; use `futures_util::future::BoxFuture`, already in the crate list):
  `pub trait EmailClient: Send + Sync { fn send(&self, message: EmailMessage) -> BoxFuture<'_, std::result::Result<(), EmailError>>; fn as_any(&self) -> &dyn std::any::Any; }`
- `EmailMessage { to: String, subject: String, text: String }` (`Clone`, `Debug`, `PartialEq`). Subjects: `"You have been invited to Mars"`, `"Reset your Mars password"`, `"Task #<number> needs a human: <title>"`.
- Link formats (frontend routes from `SPEC.md` "Frontend"): invitation `<PUBLIC_URL>/invite/<token>`, reset `<PUBLIC_URL>/reset-password/<token>`, escalation `<PUBLIC_URL>/projects/<project_id>/tasks/<number>`. Templates take the finished link; callers build it from `Config.public_url` (trailing slash trimmed).
- `EmailError` variants: `Configuration(String)`, `Transport(String)`, `Provider { status: u16 }`. `Provider` carries no response body.
- `ResendClient::new(api_key: String, from: String)` plus a `with_base_url(url)` constructor used only by tests so a local fake endpoint can stand in for Resend; the `reqwest::Client` is built once with a timeout (10 s).
- `LogEmailClient` record: `tracing::info!(to = %message.to, subject = %message.subject, text = %message.text, "email delivery is not configured; message written to the log (ADR 0026)")`. This is the one sanctioned place a token-bearing link is logged.
- `ResendClient` may log `to` and `subject` at `debug` and the provider status at `error`; never `text`.

## Edge cases
- `RESEND_API_KEY` set but `MAIL_FROM` missing: `Config::from_env()` / startup fails fast naming `MAIL_FROM` (fits the existing "fails fast naming any missing required variable" rule).
- Empty recipient: return `EmailError::Configuration`.
- Escalation recipients that opted out (`notify_email = false`) are filtered by the caller (tracker epic), not by this module.
- The mock must be `Send + Sync` (`Mutex<Vec<EmailMessage>>`, `AtomicBool` for `fail_next`).

## Testing
- Unit tests in `email/mod.rs`: each template contains its link, subject and (for escalation) project name, `#<number>`, title and reason.
- Unit test for `LogEmailClient`: install a `tracing_subscriber::fmt` subscriber with a shared in-memory writer via `tracing::subscriber::with_default`, send an invitation message, assert the captured output contains the full link.
- Unit test for `ResendClient` against an in-process axum server on an ephemeral port (`with_base_url`): assert the request body and bearer header, assert a 2xx returns `Ok`, a 500 returns `EmailError::Provider { status: 500 }`, and with the same in-memory tracing writer assert the captured log output does not contain the link or token in either case.
- Unit test for `MockEmailClient`: `sent()` returns messages in order; `fail_next()` makes exactly one send fail.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (`ARCHITECTURE.md` "Orchestrator internals", `README.md` "Configuration", ADR 0014, ADR 0026).

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `Config` with `public_url`, `resend_api_key: Option<String>`, `mail_from: Option<String>`; `AppState` and the prelude `Error` enum with an `EmailError` slot.
- "Database schema, models, repositories and test harness": `TestApp::spawn()` installs the mock email client into `AppState` and exposes it (e.g. `TestApp::email() -> &MockEmailClient` via `as_any()`); if that epic stubbed the trait, this task replaces the stub with the final signature.