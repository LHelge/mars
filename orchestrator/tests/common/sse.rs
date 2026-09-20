//! Reading Server-Sent Events in a test (`SPEC.md`, "SSE: task stream").
//!
//! `TestApp::sse` hands back the raw bytes of an open response; this module is
//! the other half: the wire format, parsed into the four things a frame can
//! carry, and the small reader that waits for the next one.
//!
//! The parser is deliberately literal rather than complete. An SSE body is
//! blocks separated by a blank line, and every block this orchestrator writes
//! is either `id:` + `event:` + `data:` or the bare comment `: keepalive`, so
//! a test that reads something else has found a bug worth failing on — which
//! is why unknown field names are kept in neither the struct nor a bag, and
//! multi-line `data:` is joined with a newline the way the specification says
//! it is.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};

/// One SSE block, as the fields this stream uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseFrame {
    /// `id: <seq>` — the `Last-Event-ID` a reconnect would send.
    pub id: Option<String>,
    /// `event: task`.
    pub event: Option<String>,
    /// `data: <TaskEvent JSON>`, lines joined with `\n`.
    pub data: Option<String>,
    /// A comment block: the text after `:`, so `: keepalive` is
    /// `Some("keepalive")`.
    pub comment: Option<String>,
}

impl SseFrame {
    /// Parse one block — everything between two blank lines.
    pub fn parse(block: &str) -> Self {
        let mut frame = Self::default();

        for line in block.lines() {
            if let Some(comment) = line.strip_prefix(':') {
                frame.comment = Some(comment.trim_start().to_string());
                continue;
            }

            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.strip_prefix(' ').unwrap_or(value).to_string();

            match field {
                "id" => frame.id = Some(value),
                "event" => frame.event = Some(value),
                "data" => match &mut frame.data {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(&value);
                    }
                    None => frame.data = Some(value),
                },
                _ => {}
            }
        }

        frame
    }

    /// The `data` of a frame that must have one, deserialised.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        let data = self.data.as_deref().expect("the frame carries data");

        serde_json::from_str(data).expect("the data is the documented JSON")
    }

    /// Whether this is the keepalive comment and nothing else.
    pub fn is_keepalive(&self) -> bool {
        self.comment.as_deref() == Some("keepalive")
            && self.id.is_none()
            && self.event.is_none()
            && self.data.is_none()
    }
}

/// A frame-at-a-time reader over the bytes of an open SSE response.
pub struct SseReader<S> {
    stream: S,
    buffer: String,
}

impl<S> SseReader<S>
where
    S: Stream<Item = Bytes> + Unpin,
{
    /// Wrap the byte stream `TestApp::sse` returned.
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: String::new(),
        }
    }

    /// The next frame, or `None` when the response body ended.
    pub async fn next_frame(&mut self) -> Option<SseFrame> {
        loop {
            if let Some(index) = self.buffer.find("\n\n") {
                let block = self.buffer[..index].to_string();
                self.buffer.drain(..index + 2);

                if !block.trim().is_empty() {
                    return Some(SseFrame::parse(&block));
                }
                continue;
            }

            let chunk = self.stream.next().await?;
            self.buffer
                .push_str(std::str::from_utf8(&chunk).expect("the body is UTF-8"));
        }
    }

    /// The next frame that is not a keepalive comment, within `within`.
    ///
    /// Keepalives are filtered because they arrive on their own timer and a
    /// scenario about events must not depend on where that timer happens to
    /// fall.
    pub async fn event_within(&mut self, within: Duration) -> SseFrame {
        tokio::time::timeout(within, async {
            loop {
                let frame = self.next_frame().await.expect("the stream is still open");
                if !frame.is_keepalive() {
                    return frame;
                }
            }
        })
        .await
        .expect("a frame arrives in time")
    }

    /// The next keepalive comment, within `within`.
    pub async fn keepalive_within(&mut self, within: Duration) -> SseFrame {
        tokio::time::timeout(within, async {
            loop {
                let frame = self.next_frame().await.expect("the stream is still open");
                if frame.is_keepalive() {
                    return frame;
                }
            }
        })
        .await
        .expect("a keepalive arrives in time")
    }

    /// Whether the body ends within `within`, draining whatever it sends
    /// first.
    pub async fn ended_within(&mut self, within: Duration) -> bool {
        tokio::time::timeout(within, async { while self.next_frame().await.is_some() {} })
            .await
            .is_ok()
    }
}

/// Every frame's `id`, in arrival order, until one reaching `until_seq` has
/// been seen.
///
/// The SSE half of `common::app::collect_ws_events`, and the same contract:
/// keepalives are skipped, every other frame must carry an `id` that is a
/// sequence, and a body that ends or a wait that runs out is a failure rather
/// than a hang.
pub async fn collect_sse_ids<S>(
    reader: &mut SseReader<S>,
    until_seq: i64,
    within: Duration,
) -> Vec<i64>
where
    S: Stream<Item = Bytes> + Unpin,
{
    let mut seen: Vec<i64> = Vec::new();

    let collected = tokio::time::timeout(within, async {
        while seen.iter().copied().max().unwrap_or(0) < until_seq {
            let frame = reader
                .next_frame()
                .await
                .unwrap_or_else(|| panic!("the stream ended before sequence {until_seq}"));

            if frame.is_keepalive() {
                continue;
            }

            seen.push(
                frame
                    .id
                    .as_deref()
                    .expect("every task frame carries an id")
                    .parse::<i64>()
                    .expect("the id is a sequence"),
            );
        }
    })
    .await;

    collected.unwrap_or_else(|_| {
        panic!("sequence {until_seq} did not arrive within {within:?}; saw {seen:?}")
    });

    seen
}
