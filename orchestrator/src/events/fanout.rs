//! The in-process fan-out every WebSocket and SSE subscriber waits on.
//!
//! `ARCHITECTURE.md`, "Event delivery": "A shared Postgres listener forwards
//! delivered notifications to the in-process broadcast channels". ADR 0005
//! fixes why the mirror exists: "Inside the orchestrator a
//! `tokio::sync::broadcast` fan-out mirrors the notification so that
//! in-process subscribers do not each hold a Postgres listener connection".
//!
//! Most subscribers watch one session or one project. One does not: a
//! consumer whose interest is a kind of event rather than an id — the
//! dispatcher's waker (`cron/dispatcher.rs`) — takes
//! [`EventFanout::subscribe_all`] and sees every notice, under exactly the
//! same rules.
//!
//! Nothing here carries event content. A [`Notice`] is an identifier and a
//! cursor, exactly like the notification payloads in `docs/data-model.md`,
//! "Notifications (LISTEN/NOTIFY channels)", and a subscriber that receives
//! one reads the rows it is missing from the table (ADRs 0005, 0028).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Mutex;

use serde::Deserialize;
use serde::de::IntoDeserializer;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::models::session::SessionState;
use crate::prelude::*;

/// How many notices a broadcast channel buffers per subscriber.
///
/// Small on purpose: a notice is a wake signal, so a subscriber that falls
/// this far behind loses nothing by being told to catch up with one read
/// instead of replaying 64 identical prompts to read.
pub const CHANNEL_CAPACITY: usize = 64;

/// A wake signal for one session or one project stream.
///
/// A notice never carries event content: it says "there may be new rows after
/// the last `seq` you saw" and the subscriber reads from its cursor
/// (`docs/data-model.md`, "Notifications (LISTEN/NOTIFY channels)"; ADR 0005).
///
/// Because of that, a subscriber treats
/// [`RecvError::Lagged`](broadcast::error::RecvError::Lagged) *exactly* like a
/// notice: it does one cursor read and carries on. Dropped notices cannot lose
/// information that the next read does not recover.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// New rows in `events` for this session, up to at least `seq`.
    SessionEvents {
        /// The highest sequence the writing transaction knew about.
        seq: i64,
    },
    /// The session's `state` column changed.
    SessionState {
        /// The state the session moved to.
        state: SessionState,
    },
    /// New rows in `task_events` for this project, up to at least `seq`.
    TaskEvents {
        /// The highest sequence the writing transaction knew about.
        seq: i64,
    },
    /// The listener lost its connection; read from your cursor now.
    ///
    /// Notifications issued while the listener was disconnected are gone, so
    /// every live subscriber re-reads rather than waiting for a notice that
    /// will never come.
    Resync,
}

/// The Postgres `LISTEN` channels the shared listener subscribes to.
///
/// The names are the schema contract in `docs/data-model.md`,
/// "Notifications (LISTEN/NOTIFY channels)", and are spelled once, here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// `session_events`, payload `<session_id>:<seq>`.
    SessionEvents,
    /// `task_events`, payload `<project_id>:<seq>`.
    TaskEvents,
    /// `session_state`, payload `<session_id>:<state>`.
    SessionState,
}

impl Channel {
    /// Every channel, in the order the listener subscribes to them.
    pub const ALL: [Channel; 3] = [
        Channel::SessionEvents,
        Channel::TaskEvents,
        Channel::SessionState,
    ];

    /// The channel name as `LISTEN` and `pg_notify` spell it.
    pub fn name(self) -> &'static str {
        match self {
            Channel::SessionEvents => "session_events",
            Channel::TaskEvents => "task_events",
            Channel::SessionState => "session_state",
        }
    }

    /// The channel a delivered notification names, or `None` for a channel
    /// this orchestrator does not know.
    pub fn from_name(name: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.name() == name)
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Every way a notification payload can fail to parse.
///
/// Never converted into an HTTP error: a malformed payload is a listener-side
/// fact, logged and dropped, and no request is waiting on it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PayloadError {
    /// The payload has no `:` separating the id from the rest.
    #[error("payload has no ':' separator")]
    MissingSeparator,
    /// The part before the `:` is not a UUID.
    #[error("payload id is not a UUID")]
    InvalidId,
    /// The part after the `:` is not a sequence number of at least 1.
    #[error("payload sequence is not a positive integer")]
    InvalidSeq,
    /// The part after the `:` is not a `session_state` value.
    #[error("payload state is not a session state")]
    InvalidState,
}

/// Parse one delivered notification payload into the id it concerns and the
/// notice to publish.
///
/// The payload shapes are `docs/data-model.md`, "Notifications (LISTEN/NOTIFY
/// channels)": `<session_id>:<seq>`, `<project_id>:<seq>` and
/// `<session_id>:<state>`. Only the first `:` separates; anything else belongs
/// to the right-hand part and makes it invalid.
pub fn parse_payload(
    channel: Channel,
    payload: &str,
) -> std::result::Result<(Uuid, Notice), PayloadError> {
    let (id, rest) = payload
        .split_once(':')
        .ok_or(PayloadError::MissingSeparator)?;
    let id = Uuid::parse_str(id).map_err(|_| PayloadError::InvalidId)?;

    let notice = match channel {
        Channel::SessionEvents => Notice::SessionEvents {
            seq: parse_seq(rest)?,
        },
        Channel::TaskEvents => Notice::TaskEvents {
            seq: parse_seq(rest)?,
        },
        Channel::SessionState => Notice::SessionState {
            state: parse_state(rest)?,
        },
    };

    Ok((id, notice))
}

/// A sequence number as the writers spell it: a decimal `i64` of at least 1,
/// because sequences start at 1.
fn parse_seq(raw: &str) -> std::result::Result<i64, PayloadError> {
    match raw.parse::<i64>() {
        Ok(seq) if seq >= 1 => Ok(seq),
        _ => Err(PayloadError::InvalidSeq),
    }
}

/// A `session_state` value, through `SessionState`'s own serde, so the list of
/// states lives in `models::session` alone.
fn parse_state(raw: &str) -> std::result::Result<SessionState, PayloadError> {
    let deserializer: serde::de::value::StrDeserializer<serde::de::value::Error> =
        raw.into_deserializer();
    SessionState::deserialize(deserializer).map_err(|_| PayloadError::InvalidState)
}

/// A notice together with the id it concerns, for a subscriber that watches
/// every session and project rather than one of them.
///
/// `id` is `None` for [`Notice::Resync`], which concerns all of them at once:
/// the listener reconnected and every subscriber owes itself a read.
#[derive(Debug, Clone, PartialEq)]
pub struct AnyNotice {
    /// The session id or project id the notice is about.
    pub id: Option<Uuid>,
    /// The notice itself; its variant says which of the two `id` is.
    pub notice: Notice,
}

/// The channels, one per subscribed id, plus the one that carries everything.
///
/// Both maps are lazily filled by `subscribe_*` and emptied again by the first
/// `publish_*` that finds no receivers left, so the fan-out is proportional to
/// what is being watched now, not to everything ever watched.
struct Inner {
    sessions: HashMap<Uuid, broadcast::Sender<Notice>>,
    projects: HashMap<Uuid, broadcast::Sender<Notice>>,
    /// Every notice, whichever id it concerns ([`EventFanout::subscribe_all`]).
    ///
    /// One channel rather than a map, created with the fan-out rather than on
    /// the first subscriber: a `broadcast::Sender` with no receivers costs one
    /// allocation and makes every `send` an ignored error, which is cheaper
    /// than the `Option` that would avoid it.
    all: broadcast::Sender<AnyNotice>,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            sessions: HashMap::new(),
            projects: HashMap::new(),
            all: broadcast::Sender::new(CHANNEL_CAPACITY),
        }
    }
}

/// The in-process broadcast fan-out `AppState` holds.
///
/// One `tokio::sync::broadcast` channel per subscribed session id and per
/// subscribed project id. The shared Postgres listener publishes; the
/// WebSocket and SSE handlers subscribe. Cloning shares the maps.
///
/// There is no async code here, and the mutex is never held across an
/// `.await`: every method locks, does map work, and unlocks before sending.
#[derive(Clone, Default)]
pub struct EventFanout {
    inner: Arc<Mutex<Inner>>,
}

impl EventFanout {
    /// An empty fan-out with no channels.
    pub fn new() -> Self {
        Self::default()
    }

    /// A receiver for this session's notices, creating the channel if this is
    /// the first subscriber.
    pub fn subscribe_session(&self, session_id: Uuid) -> broadcast::Receiver<Notice> {
        let mut inner = self.lock();
        subscribe(&mut inner.sessions, session_id)
    }

    /// A receiver for this project's notices, creating the channel if this is
    /// the first subscriber.
    pub fn subscribe_project(&self, project_id: Uuid) -> broadcast::Receiver<Notice> {
        let mut inner = self.lock();
        subscribe(&mut inner.projects, project_id)
    }

    /// A receiver for *every* notice, whichever session or project it names.
    ///
    /// For a subscriber that is not a stream: it watches no particular id and
    /// cannot subscribe to one, because the ids it cares about are whatever
    /// the notifications turn out to name. The dispatcher's waker is the one
    /// in v2 (`cron/dispatcher.rs`).
    ///
    /// Nothing here is different in kind from the per-id channels: the notices
    /// are still wake signals, a receiver still treats
    /// [`RecvError::Lagged`](broadcast::error::RecvError::Lagged) exactly like
    /// a notice, and a subscriber that wants content reads rows.
    pub fn subscribe_all(&self) -> broadcast::Receiver<AnyNotice> {
        self.lock().all.subscribe()
    }

    /// Publish to this session's subscribers; a no-op when nobody subscribes.
    pub fn publish_session(&self, session_id: Uuid, notice: Notice) {
        self.publish_all(Some(session_id), notice.clone());
        self.publish(Map::Sessions, session_id, notice);
    }

    /// Publish to this project's subscribers; a no-op when nobody subscribes.
    pub fn publish_project(&self, project_id: Uuid, notice: Notice) {
        self.publish_all(Some(project_id), notice.clone());
        self.publish(Map::Projects, project_id, notice);
    }

    /// Tell every live subscriber to read from its cursor now.
    ///
    /// What the shared listener calls when it has reconnected: notifications
    /// issued while it was away are lost, and only the subscribers' own reads
    /// can recover them. A no-op on an empty fan-out.
    pub fn publish_resync(&self) {
        // Once, not once per id: a resync says "read now" and the whole-fanout
        // subscriber has one thing to read.
        self.publish_all(None, Notice::Resync);

        let (sessions, projects) = {
            let inner = self.lock();
            let sessions: Vec<_> = inner.sessions.keys().copied().collect();
            let projects: Vec<_> = inner.projects.keys().copied().collect();
            (sessions, projects)
        };

        for id in sessions {
            self.publish(Map::Sessions, id, Notice::Resync);
        }
        for id in projects {
            self.publish(Map::Projects, id, Notice::Resync);
        }
    }

    /// How many receivers this session's channel has, `0` when there is none.
    pub fn session_subscribers(&self, session_id: Uuid) -> usize {
        self.lock()
            .sessions
            .get(&session_id)
            .map_or(0, broadcast::Sender::receiver_count)
    }

    /// How many receivers this project's channel has, `0` when there is none.
    pub fn project_subscribers(&self, project_id: Uuid) -> usize {
        self.lock()
            .projects
            .get(&project_id)
            .map_or(0, broadcast::Sender::receiver_count)
    }

    /// Send on one channel, then forget it if it has lost its last receiver.
    ///
    /// The sender is cloned out and the guard dropped before the send, so the
    /// mutex covers map work only. `send` failing means there were no
    /// receivers at that instant; the entry is removed only if a new
    /// subscriber has not appeared in the meantime.
    fn publish(&self, map: Map, id: Uuid, notice: Notice) {
        let sender = {
            let inner = self.lock();
            match map.get(&inner).get(&id) {
                Some(sender) => sender.clone(),
                None => return,
            }
        };

        if sender.send(notice).is_err() {
            let mut inner = self.lock();
            if let Entry::Occupied(entry) = map.get_mut(&mut inner).entry(id)
                && entry.get().receiver_count() == 0
            {
                entry.remove();
            }
            return;
        }

        debug!(channel = %map, id = %id, "notice published");
    }

    /// Send on the whole-fan-out channel, ignoring "nobody is listening".
    ///
    /// The sender is cloned out and the guard dropped before the send, for the
    /// same reason [`EventFanout::publish`] does it. The channel is never
    /// removed: it belongs to the fan-out and not to a subscriber.
    fn publish_all(&self, id: Option<Uuid>, notice: Notice) {
        let sender = self.lock().all.clone();
        let _ = sender.send(AnyNotice { id, notice });
    }

    /// The map guard, recovering a poisoned mutex: the only code under the
    /// lock is `HashMap` work that cannot leave a broken invariant behind.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Which of the two maps a publish concerns.
#[derive(Debug, Clone, Copy)]
enum Map {
    Sessions,
    Projects,
}

impl Map {
    fn get(self, inner: &Inner) -> &HashMap<Uuid, broadcast::Sender<Notice>> {
        match self {
            Map::Sessions => &inner.sessions,
            Map::Projects => &inner.projects,
        }
    }

    fn get_mut(self, inner: &mut Inner) -> &mut HashMap<Uuid, broadcast::Sender<Notice>> {
        match self {
            Map::Sessions => &mut inner.sessions,
            Map::Projects => &mut inner.projects,
        }
    }
}

impl std::fmt::Display for Map {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Map::Sessions => "session",
            Map::Projects => "project",
        })
    }
}

/// Subscribe to `id`'s channel in `map`, creating it on the first subscriber.
fn subscribe(
    map: &mut HashMap<Uuid, broadcast::Sender<Notice>>,
    id: Uuid,
) -> broadcast::Receiver<Notice> {
    map.entry(id)
        .or_insert_with(|| broadcast::Sender::new(CHANNEL_CAPACITY))
        .subscribe()
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast::error::{RecvError, TryRecvError};

    use super::*;

    const ID: &str = "0f9d8c7b-6a5e-4d3c-2b1a-0f9e8d7c6b5a";

    fn id() -> Uuid {
        Uuid::parse_str(ID).expect("a fixed, obviously fake id parses")
    }

    #[test]
    fn channel_names_are_the_schema_contract() {
        assert_eq!(Channel::SessionEvents.name(), "session_events");
        assert_eq!(Channel::TaskEvents.name(), "task_events");
        assert_eq!(Channel::SessionState.name(), "session_state");

        for channel in Channel::ALL {
            assert_eq!(Channel::from_name(channel.name()), Some(channel));
        }
        assert_eq!(Channel::from_name("events"), None);
    }

    #[test]
    fn a_session_events_payload_parses_into_its_cursor() {
        let payload = format!("{ID}:7");
        assert_eq!(
            parse_payload(Channel::SessionEvents, &payload),
            Ok((id(), Notice::SessionEvents { seq: 7 }))
        );
    }

    #[test]
    fn a_task_events_payload_parses_into_its_cursor() {
        let payload = format!("{ID}:1");
        assert_eq!(
            parse_payload(Channel::TaskEvents, &payload),
            Ok((id(), Notice::TaskEvents { seq: 1 }))
        );
    }

    #[test]
    fn a_session_state_payload_parses_every_state() {
        for state in [
            SessionState::Creating,
            SessionState::Running,
            SessionState::Parked,
            SessionState::Done,
            SessionState::Failed,
        ] {
            let payload = format!("{ID}:{}", state.as_str());
            assert_eq!(
                parse_payload(Channel::SessionState, &payload),
                Ok((id(), Notice::SessionState { state }))
            );
        }
    }

    #[test]
    fn an_upper_case_uuid_is_accepted() {
        let payload = format!("{}:3", ID.to_uppercase());
        assert_eq!(
            parse_payload(Channel::SessionEvents, &payload),
            Ok((id(), Notice::SessionEvents { seq: 3 }))
        );
    }

    #[test]
    fn every_payload_error_has_its_case() {
        assert_eq!(
            parse_payload(Channel::SessionEvents, ""),
            Err(PayloadError::MissingSeparator)
        );
        assert_eq!(
            parse_payload(Channel::SessionEvents, &format!("{ID}42")),
            Err(PayloadError::MissingSeparator)
        );
        assert_eq!(
            parse_payload(Channel::SessionEvents, "not-a-uuid:4"),
            Err(PayloadError::InvalidId)
        );
        assert_eq!(
            parse_payload(Channel::TaskEvents, "not-a-uuid:4"),
            Err(PayloadError::InvalidId)
        );
        // Extra separators belong to the right-hand part, which is then not a
        // sequence number.
        assert_eq!(
            parse_payload(Channel::SessionEvents, &format!("{ID}:12:34")),
            Err(PayloadError::InvalidSeq)
        );
        for seq in ["0", "-1", "", "seven"] {
            assert_eq!(
                parse_payload(Channel::TaskEvents, &format!("{ID}:{seq}")),
                Err(PayloadError::InvalidSeq),
                "sequences start at 1, so {seq:?} is not one"
            );
        }
        for state in ["", "sleeping", "Running", "running:x"] {
            assert_eq!(
                parse_payload(Channel::SessionState, &format!("{ID}:{state}")),
                Err(PayloadError::InvalidState),
                "{state:?} is not a session state"
            );
        }
    }

    #[test]
    fn two_subscribers_of_one_session_share_one_channel() {
        let fanout = EventFanout::new();
        let session = Uuid::new_v4();
        let other = Uuid::new_v4();

        let mut first = fanout.subscribe_session(session);
        let mut second = fanout.subscribe_session(session);
        let mut elsewhere = fanout.subscribe_session(other);

        assert_eq!(fanout.session_subscribers(session), 2);
        assert_eq!(fanout.session_subscribers(other), 1);

        fanout.publish_session(session, Notice::SessionEvents { seq: 9 });

        assert_eq!(first.try_recv(), Ok(Notice::SessionEvents { seq: 9 }));
        assert_eq!(second.try_recv(), Ok(Notice::SessionEvents { seq: 9 }));
        assert_eq!(elsewhere.try_recv(), Err(TryRecvError::Empty));

        // Dropping one receiver leaves the other's channel alone.
        drop(second);
        assert_eq!(fanout.session_subscribers(session), 1);
        fanout.publish_session(
            session,
            Notice::SessionState {
                state: SessionState::Parked,
            },
        );
        assert_eq!(
            first.try_recv(),
            Ok(Notice::SessionState {
                state: SessionState::Parked
            })
        );
    }

    #[test]
    fn project_channels_are_independent_of_session_channels() {
        let fanout = EventFanout::new();
        let id = Uuid::new_v4();

        let mut session = fanout.subscribe_session(id);
        let mut project = fanout.subscribe_project(id);

        fanout.publish_project(id, Notice::TaskEvents { seq: 4 });

        assert_eq!(project.try_recv(), Ok(Notice::TaskEvents { seq: 4 }));
        assert_eq!(session.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(fanout.session_subscribers(id), 1);
        assert_eq!(fanout.project_subscribers(id), 1);
    }

    #[test]
    fn publishing_to_an_unwatched_key_is_a_no_op() {
        let fanout = EventFanout::new();

        fanout.publish_session(Uuid::new_v4(), Notice::SessionEvents { seq: 1 });
        fanout.publish_project(Uuid::new_v4(), Notice::TaskEvents { seq: 1 });

        assert_eq!(fanout.session_subscribers(Uuid::new_v4()), 0);
        assert_eq!(fanout.project_subscribers(Uuid::new_v4()), 0);
    }

    #[test]
    fn the_entry_is_removed_after_the_last_receiver_is_dropped_and_a_publish_happens() {
        let fanout = EventFanout::new();
        let session = Uuid::new_v4();
        let project = Uuid::new_v4();

        let receiver = fanout.subscribe_session(session);
        let project_receiver = fanout.subscribe_project(project);
        drop(receiver);
        drop(project_receiver);

        assert_eq!(fanout.inner.lock().unwrap().sessions.len(), 1);
        assert_eq!(fanout.inner.lock().unwrap().projects.len(), 1);

        fanout.publish_session(session, Notice::SessionEvents { seq: 1 });
        fanout.publish_project(project, Notice::TaskEvents { seq: 1 });

        assert!(fanout.inner.lock().unwrap().sessions.is_empty());
        assert!(fanout.inner.lock().unwrap().projects.is_empty());

        // A later subscriber simply gets a fresh channel.
        let _again = fanout.subscribe_session(session);
        assert_eq!(fanout.session_subscribers(session), 1);
    }

    #[tokio::test]
    async fn a_subscriber_that_falls_behind_is_told_it_lagged() {
        let fanout = EventFanout::new();
        let session = Uuid::new_v4();
        let mut receiver = fanout.subscribe_session(session);

        for seq in 1..=(CHANNEL_CAPACITY as i64 + 1) {
            fanout.publish_session(session, Notice::SessionEvents { seq });
        }

        assert_eq!(receiver.recv().await, Err(RecvError::Lagged(1)));
        // And the very next read is the newest buffered notice: one cursor
        // read recovers everything the drop lost.
        assert_eq!(
            receiver.recv().await,
            Ok(Notice::SessionEvents { seq: 2 }),
            "the oldest surviving notice follows the lag report"
        );
    }

    #[test]
    fn a_resync_reaches_both_a_session_and_a_project_subscriber() {
        let fanout = EventFanout::new();
        let session = Uuid::new_v4();
        let project = Uuid::new_v4();

        let mut session_rx = fanout.subscribe_session(session);
        let mut project_rx = fanout.subscribe_project(project);

        fanout.publish_resync();

        assert_eq!(session_rx.try_recv(), Ok(Notice::Resync));
        assert_eq!(project_rx.try_recv(), Ok(Notice::Resync));
    }

    #[test]
    fn a_resync_on_an_empty_fanout_is_a_no_op() {
        EventFanout::new().publish_resync();
    }

    #[test]
    fn a_whole_fanout_subscriber_sees_every_id_and_every_channel() {
        let fanout = EventFanout::new();
        let session = Uuid::new_v4();
        let project = Uuid::new_v4();

        let mut all = fanout.subscribe_all();
        // Nobody subscribes to either id: the whole-fan-out receiver is not a
        // second subscriber of theirs, it is its own.
        fanout.publish_session(session, Notice::SessionEvents { seq: 3 });
        fanout.publish_project(project, Notice::TaskEvents { seq: 8 });
        fanout.publish_resync();

        assert_eq!(
            all.try_recv(),
            Ok(AnyNotice {
                id: Some(session),
                notice: Notice::SessionEvents { seq: 3 },
            })
        );
        assert_eq!(
            all.try_recv(),
            Ok(AnyNotice {
                id: Some(project),
                notice: Notice::TaskEvents { seq: 8 },
            })
        );
        // A resync names no id and arrives once, however many ids are watched.
        assert_eq!(
            all.try_recv(),
            Ok(AnyNotice {
                id: None,
                notice: Notice::Resync,
            })
        );
        assert_eq!(all.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn publishing_with_no_whole_fanout_subscriber_is_a_no_op() {
        let fanout = EventFanout::new();
        let session = Uuid::new_v4();
        let mut receiver = fanout.subscribe_session(session);

        fanout.publish_session(session, Notice::SessionEvents { seq: 1 });

        // The per-id subscriber is unaffected by there being no other one.
        assert_eq!(receiver.try_recv(), Ok(Notice::SessionEvents { seq: 1 }));
    }

    #[test]
    fn cloning_the_fanout_shares_its_channels() {
        let fanout = EventFanout::new();
        let clone = fanout.clone();
        let session = Uuid::new_v4();

        let mut receiver = fanout.subscribe_session(session);
        clone.publish_session(session, Notice::SessionEvents { seq: 2 });

        assert_eq!(receiver.try_recv(), Ok(Notice::SessionEvents { seq: 2 }));
        assert_eq!(clone.session_subscribers(session), 1);
    }
}
