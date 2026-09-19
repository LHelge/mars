//! The one Postgres `LISTEN` connection the process holds.
//!
//! `ARCHITECTURE.md`, "Event delivery": "A shared Postgres listener forwards
//! delivered notifications to the in-process broadcast channels; writers do
//! not broadcast before commit or send a second notification afterwards". This
//! module is that listener, and it is the only `LISTEN` in the orchestrator:
//! WebSocket and SSE handlers subscribe to [`EventFanout`], never to a
//! `PgListener` of their own (ADR 0005).
//!
//! It carries no content. A delivered notification is parsed into an id and a
//! [`Notice`] by [`parse_payload`] and published on the fan-out; the
//! subscriber reads the rows it is missing from its own cursor
//! (`docs/data-model.md`, "Notifications (LISTEN/NOTIFY channels)").
//!
//! **Losing notifications is allowed and reconnecting is not silent.** ADR
//! 0005 says a notification issued while the listener is disconnected is
//! dropped and "their loss is harmless", because cursor replay and the
//! periodic safety read recover the rows. What would not be harmless is a
//! subscriber waiting 30 seconds for that safety read, so every reconnection —
//! sqlx's own, or this module's after an error — is followed by
//! [`EventFanout::publish_resync`], which tells every live subscriber to read
//! now. That is why the loop uses `try_recv` and not `recv`: `recv` reconnects
//! underneath and would hide the gap.

use std::time::Duration;

use sqlx::postgres::{PgListener, PgNotification};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::events::fanout::{Channel, EventFanout, parse_payload};
use crate::prelude::*;

/// The delay after the first failure; every further one doubles it.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// The ceiling the backoff doubles up to.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// How long [`spawn_listener`] waits for the first `LISTEN` before letting the
/// caller serve anyway.
const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Start the shared listener and wait, briefly, until it is listening.
///
/// The returned task never panics and never returns on its own: it reconnects
/// through every failure and only returns once `shutdown` is cancelled, at
/// which point it drops its connection. Awaiting the handle after cancelling
/// is how a caller knows the connection is gone.
///
/// The await before returning is the startup ordering `run` needs: a stream
/// opened immediately after startup must not be missing notices that were
/// issued between the bind and the `LISTEN`. Waiting is bounded by
/// [`READY_TIMEOUT`] — a database that is slow to answer must not stop the API
/// from serving, and the subscribers' periodic safety read covers the gap.
pub async fn spawn_listener(
    pool: PgPool,
    fanout: EventFanout,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let (ready_tx, ready_rx) = oneshot::channel();
    let handle = tokio::spawn(listen(pool, fanout, shutdown, ready_tx));

    match tokio::time::timeout(READY_TIMEOUT, ready_rx).await {
        Ok(Ok(())) => {}
        // The task returned before it ever listened, which only a cancellation
        // during startup does.
        Ok(Err(_)) => warn!("the postgres listener stopped before it was listening"),
        Err(_) => warn!(
            timeout_secs = READY_TIMEOUT.as_secs(),
            "the postgres listener is not listening yet; serving anyway"
        ),
    }

    handle
}

/// The listener task: connect, `LISTEN`, forward, reconnect, forever.
///
/// Every await is inside a `select!` with `shutdown.cancelled()`, backoff
/// sleeps included, so a shutdown is prompt whatever the task is waiting for.
async fn listen(
    pool: PgPool,
    fanout: EventFanout,
    shutdown: CancellationToken,
    ready: oneshot::Sender<()>,
) {
    let mut ready = Some(ready);
    let mut backoff = Duration::ZERO;
    // Set the moment a connection is known to be broken and cleared by the
    // resync the next successful `LISTEN` owes every live subscriber.
    let mut owes_resync = false;

    loop {
        let mut listener = match connect(&pool, &shutdown).await {
            Some(Ok(listener)) => listener,
            Some(Err(err)) => {
                error!(error = %err, "postgres listener failed");
                backoff = next_backoff(backoff);
                if !sleep_until_retry(backoff, &shutdown).await {
                    return;
                }
                owes_resync = true;
                continue;
            }
            None => return,
        };

        // Reset only after a `listen_all` that succeeded: a connection that
        // comes up and immediately fails again keeps escalating.
        backoff = Duration::ZERO;

        if let Some(ready) = ready.take() {
            info!("postgres listener connected");
            // The receiver is gone only if the caller stopped waiting, which
            // is not this task's problem.
            let _ = ready.send(());
        }

        // `take`, so the debt is cleared by the very read that pays it.
        if std::mem::take(&mut owes_resync) {
            fanout.publish_resync();
        }

        loop {
            let received = tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                received = listener.try_recv() => received,
            };

            match received {
                Ok(Some(notification)) => publish(&fanout, &notification),
                // sqlx reconnected underneath us: whatever was issued while it
                // was away is gone (ADR 0005).
                Ok(None) => {
                    warn!("postgres listener reconnected; broadcasting resync");
                    fanout.publish_resync();
                }
                Err(err) => {
                    error!(error = %err, "postgres listener failed");
                    backoff = next_backoff(backoff);
                    if !sleep_until_retry(backoff, &shutdown).await {
                        return;
                    }
                    owes_resync = true;
                    break;
                }
            }
        }
    }
}

/// Open a connection and subscribe to every channel, unless `shutdown` wins.
///
/// `None` means cancelled; the two steps are one unit because a connection
/// without its `LISTEN` is of no use to anybody.
async fn connect(
    pool: &PgPool,
    shutdown: &CancellationToken,
) -> Option<std::result::Result<PgListener, sqlx::Error>> {
    let connect = async {
        let mut listener = PgListener::connect_with(pool).await?;
        listener
            .listen_all(Channel::ALL.into_iter().map(Channel::name))
            .await?;
        Ok(listener)
    };

    tokio::select! {
        biased;
        () = shutdown.cancelled() => None,
        result = connect => Some(result),
    }
}

/// Wait out `backoff`; `false` means the shutdown came first.
async fn sleep_until_retry(backoff: Duration, shutdown: &CancellationToken) -> bool {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => false,
        () = tokio::time::sleep(backoff) => true,
    }
}

/// The retry schedule: 500 ms, 1 s, 2 s, 4 s, 8 s, then 10 s forever.
///
/// `Duration::ZERO` is "no failure yet", which the caller resets to after a
/// `listen_all` that succeeded.
fn next_backoff(previous: Duration) -> Duration {
    if previous.is_zero() {
        INITIAL_BACKOFF
    } else {
        (previous * 2).min(MAX_BACKOFF)
    }
}

/// Parse one delivered notification and publish it on the fan-out.
///
/// A channel this orchestrator does not know and a payload that does not parse
/// are the same kind of fact: something wrote a notification this version
/// cannot read. Both are logged and dropped — no request is waiting on one, and
/// the rows are still the truth. The payload itself only ever reaches `debug`,
/// because a notification the parser rejected is not something whose content
/// belongs in an operator's log.
fn publish(fanout: &EventFanout, notification: &PgNotification) {
    let Some(channel) = Channel::from_name(notification.channel()) else {
        warn!(
            channel = %notification.channel(),
            "ignoring a notification on an unknown channel",
        );
        debug!(
            channel = %notification.channel(),
            payload = %notification.payload(),
            "the unknown channel's payload",
        );
        return;
    };

    match parse_payload(channel, notification.payload()) {
        Ok((id, notice)) => match channel {
            // A session's two channels share one fan-out channel: a subscriber
            // of a session wants both its events and its state changes.
            Channel::SessionEvents | Channel::SessionState => fanout.publish_session(id, notice),
            Channel::TaskEvents => fanout.publish_project(id, notice),
        },
        Err(err) => {
            warn!(channel = %channel, error = %err, "ignoring malformed notification");
            debug!(
                channel = %channel,
                payload = %notification.payload(),
                "the malformed notification payload",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_doubles_from_half_a_second_to_a_ten_second_cap() {
        let mut delay = next_backoff(Duration::ZERO);
        assert_eq!(delay, Duration::from_millis(500));

        for expected in [1, 2, 4, 8] {
            delay = next_backoff(delay);
            assert_eq!(delay, Duration::from_secs(expected));
        }

        // The cap is a floor on the interval, not a stop: every further
        // failure waits the same ten seconds.
        for _ in 0..3 {
            delay = next_backoff(delay);
            assert_eq!(delay, MAX_BACKOFF);
        }
    }

    #[test]
    fn a_successful_connection_resets_the_schedule() {
        let escalated = next_backoff(next_backoff(next_backoff(Duration::ZERO)));
        assert_eq!(escalated, Duration::from_secs(2));

        // What the loop does after a `listen_all` that succeeded.
        assert_eq!(next_backoff(Duration::ZERO), INITIAL_BACKOFF);
    }

    #[tokio::test]
    async fn a_cancelled_token_stops_the_backoff_sleep_at_once() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();

        assert!(
            !sleep_until_retry(MAX_BACKOFF, &shutdown).await,
            "a cancelled shutdown does not wait out the backoff"
        );
    }
}
