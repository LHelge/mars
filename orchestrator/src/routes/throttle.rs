//! Login throttling and the password-reset rate limit (`SPEC.md`,
//! "User-facing features", "Login and invites"; "REST API", row 429).
//!
//! Two small in-memory limiters, both held in `AppState` and both pure logic:
//! nothing here touches the database, the clock directly or an HTTP handler.
//! v1 runs a single orchestrator instance (`SPEC.md`, "Non-goals for v1"), so
//! process memory is the documented store — a restart clears every counter and
//! there is no table and no Redis to keep in step.
//!
//! - [`LoginThrottle`]: after [`LOGIN_FAILURE_LIMIT`] failed attempts for one
//!   username or one client address within [`LOGIN_WINDOW`], that key is
//!   blocked for [`LOGIN_BLOCK`] and the login route answers 429 without
//!   verifying the password.
//! - [`ResetRateLimit`]: at most [`RESET_LIMIT`] password-reset requests per
//!   identifier per [`RESET_WINDOW`]. Excess requests are dropped silently and
//!   the route still answers 204, so the limit is never observable from a
//!   response.
//!
//! Both take a [`Clock`] so their unit tests advance time instead of sleeping,
//! and both prune on every access, so an idle key costs nothing and the maps
//! cannot grow without bound.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::ConnectInfo;
use axum::http::HeaderMap;

use crate::prelude::*;

/// Failed attempts for one key within [`LOGIN_WINDOW`] before it is blocked.
pub const LOGIN_FAILURE_LIMIT: usize = 10;

/// The sliding window failures are counted over.
pub const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);

/// How long a key stays blocked once it reaches [`LOGIN_FAILURE_LIMIT`].
pub const LOGIN_BLOCK: Duration = Duration::from_secs(15 * 60);

/// Password-reset requests allowed per identifier per [`RESET_WINDOW`].
pub const RESET_LIMIT: usize = 3;

/// The rolling window password-reset requests are counted over.
pub const RESET_WINDOW: Duration = Duration::from_secs(60 * 60);

/// The header nginx sets in the documented deployment (`ARCHITECTURE.md`,
/// "Components").
const FORWARDED_FOR: &str = "x-forwarded-for";

/// The source of "now" for both limiters.
///
/// Injected so the unit tests step time forward without sleeping. Production
/// uses [`SystemClock`]; [`Instant`] rather than `DateTime<Utc>` because these
/// are elapsed-time windows and must not move when the wall clock does.
pub trait Clock: Send + Sync + 'static {
    /// The current monotonic instant.
    fn now(&self) -> Instant;
}

/// The production clock: the monotonic system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// The key a login failure is counted against.
///
/// The two are independent: a blocked address blocks every username from it
/// and a blocked username blocks every address, so neither an attacker
/// spraying usernames from one address nor one spraying addresses at one
/// username escapes the limit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    /// The submitted username, trimmed and case-sensitive, matching the
    /// uniqueness of `users.username` (`docs/data-model.md`).
    Username(String),
    /// The client address; see [`client_addr`].
    Address(IpAddr),
}

/// One key's failure history and its block, if any.
#[derive(Debug, Default)]
struct Entry {
    /// Failure instants inside the window, oldest first.
    failures: VecDeque<Instant>,
    /// When the block lifts, while one is in force.
    blocked_until: Option<Instant>,
}

/// A key that is blocked, and until when.
///
/// Returned by [`LoginThrottle::check`] so the caller can answer 429 and, if
/// it wants to, say how long the block still has to run. The login route
/// itself only needs to know that it must not verify the password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockedUntil {
    /// The instant the block lifts.
    pub until: Instant,
}

impl BlockedUntil {
    /// How much of the block is left at `now`; zero once it has lifted.
    pub fn retry_after(&self, now: Instant) -> Duration {
        self.until.saturating_duration_since(now)
    }
}

/// The login throttle: [`LOGIN_FAILURE_LIMIT`] failures per username or per
/// client address within [`LOGIN_WINDOW`] block that key for [`LOGIN_BLOCK`].
pub struct LoginThrottle {
    clock: Arc<dyn Clock>,
    entries: Mutex<HashMap<Key, Entry>>,
}

impl LoginThrottle {
    /// A throttle on the system clock.
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }

    /// A throttle on an injected clock, for tests.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// `Err` while either the username or the client address is blocked.
    ///
    /// The caller answers 429 without verifying the password, so a blocked key
    /// costs one hash-free request and never extends its own block.
    pub fn check(&self, username: &str, addr: IpAddr) -> std::result::Result<(), BlockedUntil> {
        let now = self.clock.now();
        let mut entries = self.entries();
        prune_logins(&mut entries, now);

        for key in keys(username, addr) {
            if let Some(until) = entries.get(&key).and_then(|entry| entry.blocked_until) {
                return Err(BlockedUntil { until });
            }
        }

        Ok(())
    }

    /// Count one failed attempt against both keys, blocking either that
    /// reaches [`LOGIN_FAILURE_LIMIT`] inside [`LOGIN_WINDOW`].
    ///
    /// A key that is already blocked is left alone: a blocked request must not
    /// push its own block further out.
    pub fn record_failure(&self, username: &str, addr: IpAddr) {
        let now = self.clock.now();
        let mut entries = self.entries();
        prune_logins(&mut entries, now);

        for key in keys(username, addr) {
            let entry = entries.entry(key).or_default();
            if entry.blocked_until.is_some() {
                continue;
            }

            entry.failures.push_back(now);
            if entry.failures.len() >= LOGIN_FAILURE_LIMIT {
                // The counter is spent on the block: when it lifts, the key
                // starts again from zero rather than from one failure below
                // the limit.
                entry.failures.clear();
                entry.blocked_until = Some(now + LOGIN_BLOCK);
            }
        }
    }

    /// Clear one username's failures after a successful login.
    ///
    /// The address counter is deliberately left alone: a successful login says
    /// nothing about the other usernames tried from that address.
    pub fn record_success(&self, username: &str) {
        let now = self.clock.now();
        let mut entries = self.entries();
        prune_logins(&mut entries, now);

        let key = Key::Username(username.trim().to_string());
        if let Some(entry) = entries.get_mut(&key) {
            entry.failures.clear();
            if entry.blocked_until.is_none() {
                entries.remove(&key);
            }
        }
    }

    /// Forget every key. For tests that reuse one app across scenarios.
    pub fn reset(&self) {
        self.entries().clear();
    }

    /// How many keys are currently tracked. Tests assert that pruning frees
    /// them; nothing in production reads it.
    pub fn tracked_keys(&self) -> usize {
        let now = self.clock.now();
        let mut entries = self.entries();
        prune_logins(&mut entries, now);
        entries.len()
    }

    /// The map, recovering from a poisoned lock rather than propagating a
    /// panic: a throttle that has lost its counters is still a throttle, and
    /// failing every login because one task panicked would be worse.
    fn entries(&self) -> MutexGuard<'_, HashMap<Key, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self::new()
    }
}

/// The two keys one attempt is counted against.
fn keys(username: &str, addr: IpAddr) -> [Key; 2] {
    [
        Key::Username(username.trim().to_string()),
        Key::Address(addr),
    ]
}

/// Drop failures that have fallen out of the window and blocks that have
/// lifted, then forget every key that has neither left.
fn prune_logins(entries: &mut HashMap<Key, Entry>, now: Instant) {
    entries.retain(|_, entry| {
        while entry
            .failures
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= LOGIN_WINDOW)
        {
            entry.failures.pop_front();
        }

        if entry.blocked_until.is_some_and(|until| until <= now) {
            entry.blocked_until = None;
        }

        !entry.failures.is_empty() || entry.blocked_until.is_some()
    });
}

/// The password-reset rate limit: [`RESET_LIMIT`] requests per identifier per
/// [`RESET_WINDOW`].
///
/// Requests are counted for identifiers that match no user too. Counting only
/// the ones that exist would make the limiter itself an oracle for whether an
/// identifier is known, which is exactly what the endpoint's unconditional 204
/// is there to hide (`SPEC.md`, "Auth (`/api/auth`)").
pub struct ResetRateLimit {
    clock: Arc<dyn Clock>,
    entries: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl ResetRateLimit {
    /// A limiter on the system clock.
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }

    /// A limiter on an injected clock, for tests.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Count one request and say whether it may be acted on.
    ///
    /// `false` means "send nothing"; the caller still answers 204. The
    /// identifier is trimmed and lower-cased, so `Bob@Example.com` and
    /// `bob@example.com` share one bucket.
    pub fn allow(&self, identifier: &str) -> bool {
        let now = self.clock.now();
        let mut entries = self.entries();
        prune_resets(&mut entries, now);

        let requests = entries.entry(normalise_identifier(identifier)).or_default();
        if requests.len() >= RESET_LIMIT {
            return false;
        }

        requests.push_back(now);
        true
    }

    /// Forget every identifier. For tests that reuse one app across scenarios.
    pub fn reset(&self) {
        self.entries().clear();
    }

    /// How many identifiers are currently tracked; see
    /// [`LoginThrottle::tracked_keys`].
    pub fn tracked_keys(&self) -> usize {
        let now = self.clock.now();
        let mut entries = self.entries();
        prune_resets(&mut entries, now);
        entries.len()
    }

    /// See [`LoginThrottle::entries`] for why a poisoned lock is recovered.
    fn entries(&self) -> MutexGuard<'_, HashMap<String, VecDeque<Instant>>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for ResetRateLimit {
    fn default() -> Self {
        Self::new()
    }
}

/// One bucket per identifier, however it was capitalised or padded.
fn normalise_identifier(identifier: &str) -> String {
    identifier.trim().to_lowercase()
}

/// Drop requests that have fallen out of the window, then forget every
/// identifier with none left.
fn prune_resets(entries: &mut HashMap<String, VecDeque<Instant>>, now: Instant) {
    entries.retain(|_, requests| {
        while requests
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= RESET_WINDOW)
        {
            requests.pop_front();
        }

        !requests.is_empty()
    });
}

/// The address a login attempt is throttled against.
///
/// The leftmost `X-Forwarded-For` entry when the header is present — nginx
/// sets it in the documented deployment and is the only thing the orchestrator
/// is reachable through there — otherwise the socket peer address from
/// `ConnectInfo<SocketAddr>`, which `lib.rs` makes available by serving the API
/// with `into_make_service_with_connect_info`.
///
/// When there is neither, the loopback address is used. That is the case in the
/// integration tests, which drive the router through `axum-test`'s mock
/// transport: there is no connection and so no peer. A test that wants two
/// distinct clients sets `X-Forwarded-For` itself.
///
/// A malformed header value falls back to the peer address rather than failing;
/// an unparsable address never panics.
pub fn client_addr(headers: &HeaderMap, peer: Option<ConnectInfo<SocketAddr>>) -> IpAddr {
    forwarded_for(headers)
        .or_else(|| peer.map(|ConnectInfo(addr)| addr.ip()))
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// The leftmost `X-Forwarded-For` entry, if it parses as an address.
///
/// Both a bare address and an `address:port` pair are accepted, because proxies
/// write both.
fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let value = headers.get(FORWARDED_FOR)?.to_str().ok()?;
    let first = value.split(',').next()?.trim();

    first
        .parse::<IpAddr>()
        .ok()
        .or_else(|| first.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock the test moves by hand, so nothing sleeps.
    struct FakeClock {
        now: Mutex<Instant>,
    }

    impl FakeClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                now: Mutex::new(Instant::now()),
            })
        }

        fn advance(&self, by: Duration) {
            let mut now = self.now.lock().expect("the test clock is never poisoned");
            *now += by;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.now.lock().expect("the test clock is never poisoned")
        }
    }

    fn addr(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, last))
    }

    fn throttle() -> (Arc<FakeClock>, LoginThrottle) {
        let clock = FakeClock::new();
        let throttle = LoginThrottle::with_clock(Arc::clone(&clock) as Arc<dyn Clock>);
        (clock, throttle)
    }

    fn limiter() -> (Arc<FakeClock>, ResetRateLimit) {
        let clock = FakeClock::new();
        let limiter = ResetRateLimit::with_clock(Arc::clone(&clock) as Arc<dyn Clock>);
        (clock, limiter)
    }

    #[test]
    fn nine_failures_still_allow_a_tenth_attempt() {
        let (_clock, throttle) = throttle();

        for _ in 0..LOGIN_FAILURE_LIMIT - 1 {
            throttle.record_failure("bob", addr(1));
        }

        assert!(throttle.check("bob", addr(1)).is_ok());
    }

    #[test]
    fn the_tenth_failure_blocks_for_exactly_fifteen_minutes() {
        let (clock, throttle) = throttle();

        for _ in 0..LOGIN_FAILURE_LIMIT {
            throttle.record_failure("bob", addr(1));
        }

        let blocked = throttle
            .check("bob", addr(1))
            .expect_err("the tenth failure blocks the key");
        assert_eq!(blocked.retry_after(clock.now()), LOGIN_BLOCK);

        // Still blocked one second before the block lifts.
        clock.advance(LOGIN_BLOCK - Duration::from_secs(1));
        assert!(throttle.check("bob", addr(1)).is_err());

        // Free a second after it, with an empty counter: one more failure must
        // not block again.
        clock.advance(Duration::from_secs(2));
        assert!(throttle.check("bob", addr(1)).is_ok());
        throttle.record_failure("bob", addr(1));
        assert!(throttle.check("bob", addr(1)).is_ok());
    }

    #[test]
    fn failures_spread_wider_than_the_window_never_block() {
        let (clock, throttle) = throttle();

        for _ in 0..LOGIN_FAILURE_LIMIT * 3 {
            throttle.record_failure("bob", addr(1));
            clock.advance(LOGIN_WINDOW + Duration::from_secs(60));
        }

        assert!(throttle.check("bob", addr(1)).is_ok());
    }

    #[test]
    fn a_blocked_request_does_not_push_the_block_further_out() {
        let (clock, throttle) = throttle();

        for _ in 0..LOGIN_FAILURE_LIMIT {
            throttle.record_failure("bob", addr(1));
        }

        clock.advance(LOGIN_BLOCK - Duration::from_secs(1));
        throttle.record_failure("bob", addr(1));

        clock.advance(Duration::from_secs(2));
        assert!(
            throttle.check("bob", addr(1)).is_ok(),
            "a failure recorded while blocked must not extend the block"
        );
    }

    #[test]
    fn the_username_and_the_address_block_independently() {
        let (_clock, throttle) = throttle();

        // Ten failures for one username, each from a different address.
        for nth in 0..LOGIN_FAILURE_LIMIT {
            throttle.record_failure("bob", addr(nth as u8));
        }

        // The username is blocked from an address that has never been seen.
        assert!(throttle.check("bob", addr(200)).is_err());
        // No single address reached the limit.
        assert!(throttle.check("alice", addr(0)).is_ok());
    }

    #[test]
    fn an_address_that_sprays_usernames_is_blocked_for_all_of_them() {
        let (_clock, throttle) = throttle();

        for nth in 0..LOGIN_FAILURE_LIMIT {
            throttle.record_failure(&format!("user{nth}"), addr(1));
        }

        assert!(throttle.check("someone-else", addr(1)).is_err());
        assert!(throttle.check("someone-else", addr(2)).is_ok());
    }

    #[test]
    fn record_success_clears_the_username_but_not_the_address() {
        let (_clock, throttle) = throttle();

        // Eight failures for eight other usernames, all from one address, then
        // one for bob: the address stands at nine, one short of the limit.
        for nth in 0..LOGIN_FAILURE_LIMIT - 2 {
            throttle.record_failure(&format!("user{nth}"), addr(1));
        }
        throttle.record_failure("bob", addr(1));

        // The address has nine failures; bob has one.
        throttle.record_success("bob");

        // Bob's own counter is gone...
        throttle.record_failure("bob", addr(1));
        assert!(
            throttle.check("bob", addr(2)).is_ok(),
            "the username counter must have restarted at zero"
        );
        // ...but that last failure was the address's tenth.
        assert!(
            throttle.check("alice", addr(1)).is_err(),
            "the address counter must survive a successful login"
        );
    }

    #[test]
    fn the_username_key_is_trimmed_and_case_sensitive() {
        let (_clock, throttle) = throttle();

        for _ in 0..LOGIN_FAILURE_LIMIT {
            throttle.record_failure("  bob  ", addr(1));
        }

        assert!(throttle.check("bob", addr(2)).is_err());
        assert!(
            throttle.check("Bob", addr(2)).is_ok(),
            "usernames are unique case-sensitively, so the key is too"
        );
    }

    #[test]
    fn idle_keys_are_pruned_and_reset_forgets_everything() {
        let (clock, throttle) = throttle();

        throttle.record_failure("bob", addr(1));
        assert_eq!(throttle.tracked_keys(), 2);

        clock.advance(LOGIN_WINDOW + Duration::from_secs(1));
        assert_eq!(
            throttle.tracked_keys(),
            0,
            "a key with no failures in the window and no block must be dropped"
        );

        for _ in 0..LOGIN_FAILURE_LIMIT {
            throttle.record_failure("bob", addr(1));
        }
        assert_eq!(throttle.tracked_keys(), 2);
        throttle.reset();
        assert_eq!(throttle.tracked_keys(), 0);
        assert!(throttle.check("bob", addr(1)).is_ok());
    }

    #[test]
    fn the_reset_limiter_allows_three_per_hour() {
        let (clock, limiter) = limiter();

        for _ in 0..RESET_LIMIT {
            assert!(limiter.allow("bob@example.invalid"));
        }
        assert!(!limiter.allow("bob@example.invalid"));

        clock.advance(RESET_WINDOW - Duration::from_secs(1));
        assert!(!limiter.allow("bob@example.invalid"));

        clock.advance(Duration::from_secs(2));
        assert!(limiter.allow("bob@example.invalid"));
    }

    #[test]
    fn the_reset_identifier_is_trimmed_and_lower_cased() {
        let (_clock, limiter) = limiter();

        assert!(limiter.allow("Bob@Example.invalid"));
        assert!(limiter.allow("  bob@example.invalid  "));
        assert!(limiter.allow("BOB@EXAMPLE.INVALID"));
        assert!(!limiter.allow("bob@example.invalid"));

        assert_eq!(limiter.tracked_keys(), 1, "one bucket for one identifier");
        assert!(limiter.allow("alice@example.invalid"));
    }

    #[test]
    fn the_reset_limiter_prunes_idle_identifiers_and_resets() {
        let (clock, limiter) = limiter();

        // Counted although nothing matches it: the limiter must not reveal
        // which identifiers exist.
        assert!(limiter.allow("nobody@example.invalid"));
        assert_eq!(limiter.tracked_keys(), 1);

        clock.advance(RESET_WINDOW + Duration::from_secs(1));
        assert_eq!(limiter.tracked_keys(), 0);

        assert!(limiter.allow("nobody@example.invalid"));
        limiter.reset();
        assert_eq!(limiter.tracked_keys(), 0);
    }

    fn headers_with(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(FORWARDED_FOR, value.parse().expect("a valid header value"));
        headers
    }

    fn peer(last: u8) -> Option<ConnectInfo<SocketAddr>> {
        Some(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, last)),
            44_321,
        )))
    }

    #[test]
    fn client_addr_prefers_the_leftmost_forwarded_for_entry() {
        let headers = headers_with("203.0.113.7, 70.41.3.18, 150.172.238.178");

        assert_eq!(client_addr(&headers, peer(9)), addr(7));
    }

    #[test]
    fn client_addr_accepts_a_single_entry_a_port_and_ipv6() {
        assert_eq!(client_addr(&headers_with("203.0.113.7"), None), addr(7));
        assert_eq!(
            client_addr(&headers_with(" 203.0.113.7:8443 "), None),
            addr(7)
        );
        assert_eq!(
            client_addr(&headers_with("2001:db8::1"), None),
            "2001:db8::1".parse::<IpAddr>().expect("a valid address")
        );
    }

    #[test]
    fn a_malformed_forwarded_for_falls_back_to_the_peer() {
        for value in ["not-an-address", "", ",", "unknown, 203.0.113.7"] {
            assert_eq!(
                client_addr(&headers_with(value), peer(9)),
                IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9)),
                "{value:?} must fall back rather than fail"
            );
        }
    }

    #[test]
    fn without_a_header_the_peer_is_used_and_without_either_loopback_is() {
        assert_eq!(
            client_addr(&HeaderMap::new(), peer(9)),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9))
        );
        assert_eq!(
            client_addr(&HeaderMap::new(), None),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
    }
}
