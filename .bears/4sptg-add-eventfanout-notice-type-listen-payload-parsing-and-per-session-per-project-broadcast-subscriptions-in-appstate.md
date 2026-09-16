---
id: "4sptg"
title: "Add EventFanout: Notice type, LISTEN payload parsing and per-session/per-project broadcast subscriptions in AppState"
status: open
priority: P0
created: "2026-09-16T20:44:03.854778874Z"
updated: "2026-09-16T20:51:52.081740217Z"
tags:
  - orchestrator
  - realtime
  - core
depends_on:
  - s52qg
  - "5h3y4"
parent: h8kw9
---

## Summary
Deliver the in-process fan-out every WebSocket and SSE subscriber waits on: `EventFanout` in `orchestrator/src/events/fanout.rs`, holding one `tokio::sync::broadcast` channel per subscribed session id and per subscribed project id, the `Notice` type that travels on those channels, and the parser for the three Postgres notification payloads. Notices carry identifiers and cursors only, never event content (ADRs 0005, 0028). `AppState` gains the `fanout` field the architecture reserves for this epic.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (`events/` is "AgentEvent / TaskEvent types, notify fan-out"; `AppState` holds "the broadcast senders for event fan-out"), "Event delivery" ("A shared Postgres listener forwards delivered notifications to the in-process broadcast channels").
- `docs/data-model.md` "Notifications (LISTEN/NOTIFY channels)" (channels `session_events` payload `<session_id>:<seq>`, `task_events` payload `<project_id>:<seq>`, `session_state` payload `<session_id>:<state>`; "consumers treat it as 'there may be new rows after the last `seq` you saw' and always read from the table").
- ADR 0005 ("Inside the orchestrator a `tokio::sync::broadcast` fan-out mirrors the notification so that in-process subscribers do not each hold a Postgres listener connection"), ADR 0028.

## Acceptance criteria
- [ ] `events::fanout::Notice` is `#[derive(Debug, Clone, PartialEq)] pub enum Notice { SessionEvents { seq: i64 }, SessionState { state: SessionState }, TaskEvents { seq: i64 }, Resync }`. `Resync` means "the listener lost its connection; read from your cursor now".
- [ ] `events::fanout::Channel { SessionEvents, TaskEvents, SessionState }` with `Channel::name(self) -> &'static str` returning exactly `"session_events"`, `"task_events"`, `"session_state"`, `Channel::ALL` and `Channel::from_name(&str) -> Option<Channel>`.
- [ ] `pub fn parse_payload(channel: Channel, payload: &str) -> Result<(Uuid, Notice), PayloadError>`: splits on the first `:`; the left part must parse as a UUID; for `session_events`/`task_events` the right part must parse as an `i64 >= 1`; for `session_state` the right part must be one of `creating|running|parked|done|failed` (reuse `SessionState`'s serde or `FromStr`; do not duplicate the list). `PayloadError` is a `thiserror` enum `{ MissingSeparator, InvalidId, InvalidSeq, InvalidState }` and is never converted into an HTTP error.
- [ ] `#[derive(Clone, Default)] pub struct EventFanout` with `new()`, `subscribe_session(&self, Uuid) -> broadcast::Receiver<Notice>`, `subscribe_project(&self, Uuid) -> broadcast::Receiver<Notice>`, `publish_session(&self, Uuid, Notice)`, `publish_project(&self, Uuid, Notice)`, `publish_resync(&self)` (sends `Notice::Resync` on every live channel), `session_subscribers(&self, Uuid) -> usize`, `project_subscribers(&self, Uuid) -> usize`.
- [ ] Channels are created lazily on first `subscribe_*` with capacity 64; a `publish_*` for a key with no channel is a no-op; after a `publish_*` whose `send` reports no receivers, the entry is removed so the maps do not grow with every session ever watched.
- [ ] A subscriber that falls behind gets `broadcast::error::RecvError::Lagged`; the doc comment on `Notice` says subscribers treat `Lagged` exactly like a notice (do one cursor read), because notices carry no content.
- [ ] `AppState` gains `pub fanout: EventFanout` with `impl FromRef<AppState> for EventFanout`; `AppState::new(...)` and `TestApp::spawn()` construct it with `EventFanout::new()`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/events/fanout.rs` (new), `orchestrator/src/events/mod.rs` (`pub mod fanout; pub use fanout::{Channel, EventFanout, Notice};`), `orchestrator/src/prelude/state.rs` (field and `FromRef`), `orchestrator/tests/common/mod.rs` (construct the field).
- Internals: `Arc<std::sync::Mutex<Inner>>` with `Inner { sessions: HashMap<Uuid, broadcast::Sender<Notice>>, projects: HashMap<Uuid, broadcast::Sender<Notice>> }`. Never hold the mutex across an `.await` (this type has no async code). `publish_*` clones the sender out of the map, drops the guard, sends, and on `Err(SendError)` re-locks and removes the entry only if `receiver_count() == 0`.
- `SessionState` comes from `models::session`.
- Logging: at most `debug!(channel = %name, id = %uuid, "notice published")`; never `info` for individual notices.

## Edge cases
- Payload with extra separators (`<uuid>:12:34`) → `InvalidSeq`; empty payload → `MissingSeparator`; upper-case UUIDs are accepted by `Uuid::parse_str`.
- `seq` of 0 or negative → `InvalidSeq` (sequences start at 1).
- Two `subscribe_session` calls for the same id share one channel; dropping one receiver does not affect the other.
- `publish_resync` on an empty fanout is a no-op.

## Testing
- Unit tests in `fanout.rs`: parse every channel's happy path and each error variant; subscribe/publish delivers to two subscribers of one session and not to a subscriber of another session; project channels are independent of session channels; entry removed after the last receiver is dropped and a publish happens; `Lagged` after 65 unread notices; `publish_resync` reaches both a session and a project subscriber.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `AppState` in `prelude/state.rs` with the documented extension point for the broadcast senders.
- "Database schema, models, repositories and test harness": `models::session::SessionState` and `TestApp::spawn()` constructing `AppState`.