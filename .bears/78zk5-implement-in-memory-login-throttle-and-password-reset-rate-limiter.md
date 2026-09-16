---
id: "78zk5"
title: Implement in-memory login throttle and password-reset rate limiter
status: open
priority: P1
created: "2026-09-16T20:28:32.263229827Z"
updated: "2026-09-16T20:51:51.672140355Z"
tags:
  - orchestrator
  - auth
depends_on:
  - p5tsd
parent: qacxf
---

## Summary
Add two small, clock-injectable in-memory limiters held in `AppState`: `LoginThrottle` (10 failures per username or per client address within 15 minutes blocks that key for 15 minutes, answered as 429) and `ResetRateLimit` (3 password-reset requests per identifier per hour; excess requests are silently dropped and still answer 204). v1 runs a single orchestrator instance, so process memory is the documented store; no table or Redis.

## Documents
- `SPEC.md` "User-facing features", "Login and invites" paragraph (exact numbers).
- `SPEC.md` "REST API" status table, row 429; "Auth (`/api/auth`)" rows for `/auth/login` and `/auth/request-password-reset`.
- `SPEC.md` "Non-goals for v1" (single orchestrator instance).

## Acceptance criteria
- [ ] `LoginThrottle::check(username, addr) -> Result<(), BlockedUntil>` returns `Err` while either key is blocked; `record_failure(username, addr)` increments both keys' failure counts within a sliding 15-minute window and, when a key reaches 10, blocks it until `now + 15 min`; `record_success(username)` clears that username's failures (the address counter is left alone).
- [ ] While blocked the route answers 429 with error `too many login attempts` without verifying the password; blocked requests do not extend the block.
- [ ] `ResetRateLimit::allow(identifier) -> bool` permits at most 3 calls per normalised identifier (trimmed, lower-cased) per rolling hour and returns `false` afterwards; the caller still answers 204.
- [ ] Both types take a `Clock` (`fn now() -> Instant`/`DateTime<Utc>`) so unit tests advance time without sleeping, and prune stale entries on access so memory does not grow unbounded.
- [ ] `AppState` gains `login_throttle: Arc<LoginThrottle>` and `reset_rate_limit: Arc<ResetRateLimit>`; `TestApp` exposes a way to reset them between scenarios (fresh `TestApp` per test already does).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/throttle.rs` (new; pure logic, no I/O), `orchestrator/src/prelude/state.rs` (or wherever `AppState` lives) for the two fields, `orchestrator/src/main.rs` and `tests/common/mod.rs` construction.
- Data shape: `Mutex<HashMap<Key, Entry>>` with `enum Key { Username(String), Address(IpAddr) }` and `Entry { failures: VecDeque<Instant>, blocked_until: Option<Instant> }`; pruning drops failures older than 15 minutes and entries with no failures and no active block.
- Client address source: the leftmost entry of `X-Forwarded-For` when present (nginx sets it in the documented deployment), otherwise the socket peer address from `ConnectInfo<SocketAddr>`. Provide `client_addr(&HeaderMap, Option<ConnectInfo>) -> IpAddr` here for the login route.
- Username key is the raw submitted username trimmed (case-sensitive, matching `users.username` uniqueness); the reset identifier is trimmed and lower-cased so `Bob@Example.com` and `bob@example.com` share a bucket.
- Constants: `LOGIN_FAILURE_LIMIT = 10`, `LOGIN_WINDOW = 15 min`, `LOGIN_BLOCK = 15 min`, `RESET_LIMIT = 3`, `RESET_WINDOW = 1 h`.

## Edge cases
- Both keys block independently: a blocked address blocks every username from it; a blocked username blocks every address.
- A successful login for user A from a blocked address is still 429 (address block wins).
- Malformed `X-Forwarded-For` falls back to the peer address; an unparsable address must never panic.
- The reset limiter counts requests for unknown identifiers too (otherwise it leaks whether an identifier exists through timing of limiter growth); it never reveals the limit in the response.

## Testing
- Unit tests with a fake clock: 9 failures allow, 10th blocks for exactly 15 minutes, 15 minutes and one second later the key is free and its counter empty; failures 16 minutes apart never block; `record_success` clears the username but not the address; reset limiter allows 3, denies the 4th, allows again after an hour; pruning removes idle entries.
- `client_addr` unit tests: header present, header malformed, header absent.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "Authentication": add one sentence stating that the throttled "client address" is the leftmost `X-Forwarded-For` entry when present, otherwise the peer address, and that the throttle state is process memory (single instance). Same commit.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `AppState` struct and its construction in `main.rs`; the API listener is started with `into_make_service_with_connect_info::<SocketAddr>()` so `ConnectInfo` is available (add it in this task if missing).