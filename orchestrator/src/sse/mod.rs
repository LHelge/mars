//! The project task event stream (`SPEC.md`, "SSE: task stream").
//!
//! `GET /api/projects/{pid}/tasks/stream?token=<jwt>`, the task board's half
//! of the delivery contract `ARCHITECTURE.md`, "Event delivery" gives the
//! session socket: the same subscribe-then-replay-then-follow order, keyed by
//! project instead of by session, over Server-Sent Events instead of a
//! WebSocket.
//!
//! Two halves, and the split is the same one `ws/` makes:
//!
//! 1. **Before the response opens** everything that still has an HTTP status:
//!    the token (401 / 403), the project row (404) and the cursor (400). A
//!    browser can act on a status; it cannot act on anything sent inside a
//!    `200` body, because SSE has no error frame at all.
//! 2. **After it opens** the stream itself: subscribe to the project's fan-out
//!    channel, *then* replay from the cursor, then follow the notices. The
//!    subscription precedes the replay so a row committed while the first page
//!    is being written is either read by a later page or wakes the live loop,
//!    and the cursor advances per row, so nothing is sent twice.
//!
//! The cursor is `Last-Event-ID` when the browser reconnects with one and
//! `?after=` on a first connection, in that order of precedence (`SPEC.md`,
//! "SSE: task stream").
//!
//! The keepalive is written here rather than with `Sse::keep_alive`, and that
//! is the whole reason this handler owns its own `select!` loop:
//! `SPEC.md`, "Authentication" makes the keepalive tick the moment the stream
//! re-checks its authorization, and axum's keep-alive is a timer with nothing
//! to hang a check on. A failed check ends the response — there is no frame to
//! explain it with, so the log line is the explanation (ADR 0025).
//!
//! No `retry:` field is ever written: the browser client disables automatic
//! reconnect and reopens explicitly with a refreshed token (`SPEC.md`,
//! "Authentication"), and a `retry:` would tell `EventSource` otherwise.

use std::collections::VecDeque;
use std::convert::Infallible;

use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::{CACHE_CONTROL, HeaderName, HeaderValue};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::Stream;
use serde::Deserialize;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, Interval, interval_at};
use uuid::Uuid;

use crate::events::{Notice, TaskEvent};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::routes::{StreamPrincipal, StreamToken, authenticate_stream, reauthorize};

/// What a stream for a project that does not exist is answered with (404).
const PROJECT_NOT_FOUND: &str = "project not found";

/// What a `Last-Event-ID` or `?after=` that is not a sequence number is
/// answered with (400).
///
/// One message for both, and for a negative number as well as for something
/// that is not a number at all: none of them is a cursor, and which of the two
/// places it came from is visible in the request the caller made.
const BAD_CURSOR: &str = "invalid cursor";

/// The SSE `event:` name every task frame carries (`SPEC.md`, "SSE: task
/// stream").
const TASK_EVENT: &str = "task";

/// The keepalive comment's text, so the wire form is `: keepalive`.
const KEEPALIVE: &str = "keepalive";

/// The reconnect header `EventSource` sends, and the first place the cursor is
/// looked for.
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");

/// nginx's per-response buffering switch (`SPEC.md`, "Frontend
/// architecture"): the location is configured with `proxy_buffering off`, and
/// this header is the same instruction travelling with the response, so a
/// proxy the deployment did not configure cannot hold frames back either.
const X_ACCEL_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");

/// How many events one replay or cursor read carries.
///
/// The documented page ceiling for this stream
/// (`repositories::tasks::MAX_TASK_EVENT_PAGE`), for the reason the
/// session socket gives: the reader keeps going until a page comes back short,
/// so a larger page is fewer round trips, and the bound only exists so that a
/// client thousands of events behind is not buffered whole before the first
/// frame is written.
const PAGE: u32 = crate::repositories::tasks::MAX_TASK_EVENT_PAGE;

/// The SSE routes, merged into the `/api` router beside the other resources.
///
/// The `{pid}` capture is part of the path rather than of a `nest`, for the
/// reason `routes::mod` gives: axum takes one `nest` per prefix and
/// `/projects` already has one.
pub fn routes() -> Router<AppState> {
    Router::new().route("/projects/{pid}/tasks/stream", get(task_stream))
}

/// `?after=` alone. The token is [`StreamToken`]'s, from the same query
/// string, which is why neither struct denies unknown fields.
///
/// A `String` rather than an `i64` for the reason `ws` keeps it one: a typed
/// field would answer serde's own message about an invalid digit, and the
/// documented answer is [`BAD_CURSOR`].
#[derive(Debug, Deserialize)]
struct AfterQuery {
    after: Option<String>,
}

/// The cursor this stream replays from: `Last-Event-ID`, else `?after=`,
/// else 0.
///
/// The header wins when both are there, because it is the browser's own
/// record of the last frame it processed and `?after=` is only the value the
/// application remembered when it built the URL.
fn cursor(headers: &HeaderMap, query: &AfterQuery) -> Result<i64> {
    let from_header = headers
        .get(LAST_EVENT_ID)
        .map(|value| value.to_str().unwrap_or("").to_string());

    let raw = match from_header {
        Some(header) => header,
        None => match query.after.clone() {
            Some(after) => after,
            None => return Ok(0),
        },
    };

    match raw.trim().parse::<i64>() {
        Ok(cursor) if cursor >= 0 => Ok(cursor),
        _ => Err(Error::BadRequest(BAD_CURSOR.to_string())),
    }
}

/// `GET /projects/{pid}/tasks/stream?token=&after=` (401, 403, 404, 400, then
/// the stream).
///
/// Every check with an HTTP answer happens here, and the subscription is taken
/// before the response is built: from the moment the client can see a `200` it
/// is already attached to the project's channel.
async fn task_stream(
    State(state): State<AppState>,
    Path(pid): Path<Uuid>,
    Query(query): Query<AfterQuery>,
    headers: HeaderMap,
    token: StreamToken,
) -> Result<Response> {
    let user = authenticate_stream(&state, &token.0).await?;

    // The row before the cursor: a board watching a project that has been
    // deleted gets the 404 it can act on, whatever it asked to replay from.
    ProjectRepository::new(&state.pool)
        .find(pid)
        .await?
        .ok_or_else(|| Error::Missing(PROJECT_NOT_FOUND.to_string()))?;

    let cursor = cursor(&headers, &query)?;
    let principal = StreamPrincipal::new(&user);

    // Before the response, never after: a notification that arrives between
    // the replay's last page and the first `recv` is the one thing this order
    // exists to keep (`ARCHITECTURE.md`, "Event delivery").
    let rx = state.fanout.subscribe_project(pid);

    debug!(project_id = %pid, user_id = %user.id, "task stream opening");

    let timings = state.realtime_timings;
    let context = StreamContext {
        state,
        project_id: pid,
        principal,
        cursor,
        rx,
        pending: VecDeque::new(),
        // Neither timer fires immediately: the stream is about to read from
        // its cursor, and has nothing to keep alive before its first frame.
        safety: interval_at(Instant::now() + timings.safety_read, timings.safety_read),
        keepalive: interval_at(
            Instant::now() + timings.sse_keepalive,
            timings.sse_keepalive,
        ),
        draining: true,
        done: false,
    };

    let mut response = Sse::new(body(context)).into_response();
    let response_headers = response.headers_mut();
    // axum sets both of these already; setting them here is what makes them
    // this handler's contract rather than a detail of the response type
    // (`SPEC.md`, "SSE: task stream").
    response_headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response_headers.insert(X_ACCEL_BUFFERING, HeaderValue::from_static("no"));

    Ok(response)
}

/// One open stream's state.
///
/// `cursor` is the contract with the client: every event already written has
/// `seq <= cursor`, so every read is `seq > cursor` and no row can be sent
/// twice or skipped.
struct StreamContext {
    state: AppState,
    project_id: Uuid,
    /// Who the stream was opened as, re-checked on every keepalive tick.
    principal: StreamPrincipal,
    cursor: i64,
    /// Subscribed before the response was built and dropped with this struct,
    /// which is what lets the fan-out forget a project nobody is watching.
    rx: Receiver<Notice>,
    /// Frames read but not yet yielded.
    pending: VecDeque<Event>,
    safety: Interval,
    keepalive: Interval,
    /// Whether another page should be read before waiting for anything: `true`
    /// for the initial replay, and again whenever a notice or a safety tick
    /// says to read from the cursor.
    draining: bool,
    /// Set by the one thing that ends a stream that nobody disconnected from:
    /// a failed re-authorization, a deleted project or a read that failed.
    done: bool,
}

/// What woke the live loop.
///
/// The `select!` cannot call a method on `self` in two of its branches — both
/// would borrow it mutably — so it names which arm fired and the handling
/// happens after it.
enum Wake {
    Notice(std::result::Result<Notice, RecvError>),
    Safety,
    Keepalive,
}

/// The response body: the replay, then the live loop, as one stream.
///
/// `Infallible`, because there is no error an SSE body could carry: every
/// failure below either skips a row or ends the stream.
fn body(context: StreamContext) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    futures_util::stream::unfold(context, |mut context| async move {
        loop {
            if let Some(event) = context.pending.pop_front() {
                return Some((Ok(event), context));
            }

            if context.done {
                return None;
            }

            context.step().await;
        }
    })
}

impl StreamContext {
    /// One turn of the stream: either another page, or a wait.
    async fn step(&mut self) {
        if self.draining {
            self.read_page().await;
            return;
        }

        let wake = {
            let Self {
                rx,
                safety,
                keepalive,
                ..
            } = self;

            tokio::select! {
                notice = rx.recv() => Wake::Notice(notice),
                _ = safety.tick() => Wake::Safety,
                _ = keepalive.tick() => Wake::Keepalive,
            }
        };

        match wake {
            Wake::Notice(notice) => self.on_notice(notice),
            Wake::Safety => self.on_safety().await,
            Wake::Keepalive => self.on_keepalive().await,
        }
    }

    /// One notice from the fan-out.
    ///
    /// `Lagged` is treated exactly like a notice — the notices it dropped can
    /// only have said "read from your cursor", which is what this asks for
    /// (ADR 0005) — and never ends the stream. A session's notices reach this
    /// channel only if the fan-out is keyed wrongly, so they are ignored
    /// rather than acted on.
    fn on_notice(&mut self, notice: std::result::Result<Notice, RecvError>) {
        match notice {
            Ok(Notice::TaskEvents { .. }) | Ok(Notice::Resync) | Err(RecvError::Lagged(_)) => {
                self.draining = true;
            }
            Ok(Notice::SessionEvents { .. }) | Ok(Notice::SessionState { .. }) => {}
            // The fan-out has gone away, which only happens at shutdown.
            Err(RecvError::Closed) => self.done = true,
        }
    }

    /// The periodic safety read (`ARCHITECTURE.md`, "Event delivery"): read
    /// from the cursor whether or not a notification arrived, and check that
    /// the project is still there.
    ///
    /// The project check is here rather than on a timer of its own because a
    /// deleted project cascades its `task_events` away (`docs/data-model.md`):
    /// without it a board would sit on an open response to a project that no
    /// longer exists, and there is no frame to tell it so.
    async fn on_safety(&mut self) {
        match ProjectRepository::new(&self.state.pool)
            .find(self.project_id)
            .await
        {
            Ok(Some(_)) => self.draining = true,
            Ok(None) => {
                debug!(project_id = %self.project_id, "task stream closed: the project is gone");
                self.done = true;
            }
            Err(error) => {
                error!(project_id = %self.project_id, %error, "task stream could not read the project");
                self.done = true;
            }
        }
    }

    /// One keepalive tick: the re-authorization and the comment, in that
    /// order (`SPEC.md`, "Authentication").
    ///
    /// Both [`StreamAuthFailure`](crate::routes::StreamAuthFailure) variants
    /// end the response, including the one that means the check could not be
    /// made: a database failure must never authorize continued streaming.
    async fn on_keepalive(&mut self) {
        if let Err(failure) = reauthorize(&self.state, &self.principal).await {
            debug!(
                project_id = %self.project_id,
                user_id = %self.principal.user_id,
                %failure,
                "task stream closed by re-authorization",
            );
            self.done = true;
            return;
        }

        self.pending.push_back(Event::default().comment(KEEPALIVE));
    }

    /// Read one page from the cursor and turn it into frames.
    ///
    /// `draining` stays set while the page comes back full, so the caller
    /// keeps reading until a short page says the client has caught up. The
    /// cursor advances per row, including over a row that could not be turned
    /// into an event: one unreadable row must not stall a stream for ever.
    async fn read_page(&mut self) {
        let page = match TaskRepository::new(&self.state.pool)
            .list_task_events_after(self.project_id, self.cursor, PAGE)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                error!(project_id = %self.project_id, %error, "task stream could not read events");
                self.done = true;
                return;
            }
        };

        self.draining = page.len() == PAGE as usize;

        for row in page {
            let seq = row.seq;
            self.cursor = seq;

            // `from_row` has already logged what was wrong with it; this line
            // says what the stream did about it. Never the payload (rule 3).
            let Ok(event) = TaskEvent::from_row(row) else {
                error!(project_id = %self.project_id, seq, "task stream skipped an unreadable event");
                continue;
            };

            match serde_json::to_string(&event) {
                Ok(data) => self.pending.push_back(
                    Event::default()
                        .id(seq.to_string())
                        .event(TASK_EVENT)
                        .data(data),
                ),
                Err(error) => {
                    error!(project_id = %self.project_id, seq, %error, "a task event failed to serialise");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(after: Option<&str>) -> AfterQuery {
        AfterQuery {
            after: after.map(str::to_string),
        }
    }

    fn headers(last_event_id: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = last_event_id {
            headers.insert(
                LAST_EVENT_ID,
                HeaderValue::from_str(value).expect("a test header value is valid"),
            );
        }
        headers
    }

    #[test]
    fn no_cursor_at_all_replays_from_the_beginning() {
        assert_eq!(
            cursor(&headers(None), &query(None)).expect("no cursor is zero"),
            0
        );
    }

    #[test]
    fn after_is_the_cursor_on_a_first_connection() {
        assert_eq!(
            cursor(&headers(None), &query(Some("41"))).expect("a sequence parses"),
            41
        );
        assert_eq!(
            cursor(&headers(None), &query(Some("0"))).expect("zero parses"),
            0
        );
    }

    #[test]
    fn last_event_id_is_the_cursor_and_wins_over_after() {
        assert_eq!(
            cursor(&headers(Some("7")), &query(None)).expect("a header sequence parses"),
            7
        );
        assert_eq!(
            cursor(&headers(Some("7")), &query(Some("41"))).expect("the header wins"),
            7,
        );
    }

    #[test]
    fn a_cursor_that_is_not_a_sequence_is_the_one_bad_request() {
        for raw in ["-1", "", "seven", "1.5", "9999999999999999999999"] {
            for (headers, query) in [
                (headers(Some(raw)), query(None)),
                (headers(None), query(Some(raw))),
            ] {
                match cursor(&headers, &query).expect_err("{raw} is not a cursor") {
                    Error::BadRequest(message) => assert_eq!(message, BAD_CURSOR),
                    other => panic!("{raw:?} must be a 400, not {other:?}"),
                }
            }
        }
    }

    #[test]
    fn a_header_that_wins_is_never_silently_replaced_by_the_query() {
        // A present-but-unparsable header is still the cursor that was
        // chosen, so it is a 400 rather than a fall-through to a valid
        // `?after=`.
        let error = cursor(&headers(Some("x")), &query(Some("3")))
            .expect_err("the header is the chosen cursor");

        assert!(matches!(error, Error::BadRequest(_)));
    }
}
