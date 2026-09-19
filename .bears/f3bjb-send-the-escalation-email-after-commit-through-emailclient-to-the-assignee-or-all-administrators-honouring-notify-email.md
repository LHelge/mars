---
id: f3bjb
title: Send the escalation email after commit through EmailClient to the assignee or all administrators, honouring notify_email
status: in_progress
priority: P2
created: "2026-09-16T20:46:30.429134342Z"
updated: "2026-09-19T12:33:27.873011076Z"
tags:
  - orchestrator
  - tracker
  - auth
depends_on:
  - "2sjtz"
parent: "5h3y4"
attempts: 1
---

## Summary
Turn the `Escalation` records a committed mutation returns into email: one message per escalation to the task's assignee when it has one and has `notify_email` on, otherwise one message to every administrator with `notify_email` on, sent through `AppState.email` strictly after the transaction has committed. Provide the single `tracker::escalation::notify(state, escalations)` entry point that the `needs_human` path, `release_by_agent` and `release_leases_for_session` callers (MCP tools, session hooks, reaper) invoke, and make sure a delivery failure is logged and never fails the tracker operation.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "Notification" (every move into the human state by `needs_human` or the reaper sends one email through `EmailClient` to the assignee if any, otherwise every admin, skipping users whose `notify_email` is off; names the project, the task, the reason and a link; no other tracker change sends email), "One mutation at a time per project" (email runs outside the tracker transaction).
- `SPEC.md` "User-facing features" → "Task board" (assignee or every admin; each user can opt out), "Frontend" → routes (`/projects/:id/tasks/:number` is the link target).
- `README.md` "Configuration" (`PUBLIC_URL`; escalations go to the log without `RESEND_API_KEY`), the "Escalations to `needs_human` email..." bullet under "Operation".
- `docs/data-model.md` `users.notify_email`, `users.admin`.
- ADRs 0014, 0026.

## Acceptance criteria
- [ ] `orchestrator/src/tracker/escalation.rs` exposes `pub async fn notify(state: &AppState, escalations: Vec<Escalation>)` (returns `()`; errors are logged): for each escalation resolve recipients: if `assignee_user_id` is `Some` and that user exists with `notify_email = true` → that user only; if the assignee exists but opted out → nobody (the assignee's choice is respected; admins are not fallen back to); if `assignee_user_id` is `None` or the user no longer exists → every user with `admin = true AND notify_email = true`.
- [ ] Each recipient receives `EmailMessage::escalation(to, project_name, task_number, task_title, reason, link)` with `link = format!("{public_url}/projects/{project_id}/tasks/{number}")` (trailing slash on `PUBLIC_URL` trimmed).
- [ ] Sending happens after `TrackerMutation::commit` returned; `notify` is never called while a transaction is open (enforced by signature: it takes `&AppState` and owned `Escalation`s, not a mutation).
- [ ] A failed send logs `tracing::error!(project_id = %.., task_id = %.., recipient = %.., "escalation email failed")` and continues with the next recipient; the tracker operation that produced the escalation has already succeeded.
- [ ] `UserRepository` gains (or this task adds) `list_admin_recipients() -> Vec<User>` (`admin AND notify_email`) and `find_recipient(user_id) -> Option<User>`; no secret or token is logged.
- [ ] Callers wired in this task: the `needs_human`/`release_by_agent` domain functions return escalations through `MutationOutcome`; a helper `tracker::commit_and_notify(m, state) -> Result<MutationOutcome>` commits then calls `notify`, so routes and tools have one call.

## Implementation notes
- Files: `orchestrator/src/tracker/escalation.rs` (extend), `orchestrator/src/repositories/users.rs` (recipient queries), `orchestrator/src/tracker/mod.rs` (`commit_and_notify`).
- `Escalation` already carries `project_id`, `project_name`, `task_id`, `task_number`, `task_title`, `assignee_user_id`, `reason` (added in the mutation task); if `project_name` is missing there, add it now.
- Recipient lookup runs on the pool after commit, outside any lock.
- With `LogEmailClient` (no `RESEND_API_KEY`) the message and link land in the log at `info`, which is the sanctioned exception (ADR 0026, CLAUDE.md rule 3); this code adds no logging of the message body itself.

## Edge cases
- No administrators with `notify_email` on and no assignee → nothing is sent, logged at `debug` (`no escalation recipients`).
- The same user is the assignee and an admin → exactly one message.
- Several escalations in one `release_leases_for_session` outcome → one message per escalation per recipient (three tasks escalating to the same admin produce three messages).
- The task was deleted between commit and send: the message still goes out with the captured title and number (the link may 404; acceptable).
- `MockEmailClient::fail_next()` during a send → logged, remaining sends proceed.

## Testing
- Integration tests in `orchestrator/tests/tracker_escalation_email.rs` through `TestApp` (mock email via `as_any()`): task with an assignee who has `notify_email = true` escalates (drive three claim/release cycles or call `needs_human`) → exactly one captured message to the assignee's email containing the project name, `#<number>`, the title, the reason and `<PUBLIC_URL>/projects/<pid>/tasks/<number>`; assignee with `notify_email = false` → zero messages; no assignee, two admins (one opted out) and one non-admin → exactly one message, to the opted-in admin; `needs_human` on a task already in the human state → zero messages; a user `PUT` into `needs_human` → zero messages; `fail_next()` set before an escalation with two admin recipients → one message captured, the operation still returned success and the task is in the human state.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Task tracker" → "Notification": add the sentence that an assignee who opted out receives nothing and administrators are not used as a fallback in that case (the document is silent on it; this is the chosen reading).

## Assumes from other epics
- "Authentication, users, invites and email": `EmailClient`, `EmailMessage::escalation(to, project_name, task_number, task_title, reason, link)`, `MockEmailClient` with `sent()` and `fail_next()`, `Config.public_url`, `UserRepository`, `TestApp` user creation helpers.