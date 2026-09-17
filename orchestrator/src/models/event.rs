//! The stored half of a session's `AgentEvent` stream.
//!
//! `docs/data-model.md`, `events` and `SPEC.md`, "AgentEvent". This module is
//! the row; the delivered `AgentEvent` shape — one variant per `kind`, with
//! `seq`, `ts` and `kind` lifted back out of the columns — is the agent and
//! realtime epics', and travels in [`payload`](EventRow::payload) as
//! `serde_json::Value` until then.
//!
//! `seq` is allocated as `MAX(seq)+1` for the session while the session row is
//! locked, so it is not part of [`NewEvent`]: only the repository may set it,
//! inside the writing transaction (ADR 0021, 0028).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions"); see `session.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// The prefix marking a payload field as internal (`SPEC.md`, "AgentEvent").
///
/// Fields whose names start with it never leave the orchestrator; see
/// [`EventRow::public_payload`].
pub const INTERNAL_FIELD_PREFIX: char = '_';

/// The payload field holding the transcript byte offset a translated line
/// ended at (`SPEC.md`, "AgentEvent").
///
/// Internal by the rule above. The session owner records it with every event
/// translated from `log/stream.jsonl` and reads the highest one back after a
/// restart to know where to resume tailing (`ARCHITECTURE.md`, "Durability and
/// recovery"); `SessionRepository::max_offset` is that read.
pub const OFFSET_FIELD: &str = "_offset";

/// An `events` row, column for column (`docs/data-model.md`, `events`).
///
/// `kind` is a `String` rather than an enum because the column is `TEXT` on
/// purpose: the set of kinds is owned by `src/events/`, not by the schema, and
/// adding one must not need a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct EventRow {
    pub session_id: Uuid,
    pub seq: i64,
    pub ts: DateTime<Utc>,
    pub kind: String,
    pub payload: Value,
}

impl EventRow {
    /// The payload without its internal fields (`SPEC.md`, "AgentEvent":
    /// "Payload fields whose names start with `_` ... are internal and are
    /// stripped before an event leaves the orchestrator").
    ///
    /// A pure helper: stripping at the DTO boundary is the sessions and
    /// realtime epics' job, and this is the one place that knows the rule.
    /// Only the top level is filtered — `_`-prefixed keys nested inside a tool
    /// call's arguments are the agent's data, not ours.
    pub fn public_payload(&self) -> Value {
        match &self.payload {
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(name, _)| !name.starts_with(INTERNAL_FIELD_PREFIX))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// An event about to be appended, without the `seq` the repository allocates
/// under the session row lock.
///
/// `ts` is the time the orchestrator *observed* the event, not the time the
/// row was written, so it is the caller's: a batch translated from one native
/// line shares the observation time of that line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    pub ts: DateTime<Utc>,
    pub kind: String,
    pub payload: Value,
}

impl NewEvent {
    /// An event observed now.
    pub fn now(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            ts: Utc::now(),
            kind: kind.into(),
            payload,
        }
    }

    /// The same event with the transcript offset the line it came from ended
    /// at, recorded as the internal [`OFFSET_FIELD`].
    ///
    /// A no-op on a payload that is not a JSON object, which no event kind
    /// produces.
    pub fn with_offset(mut self, offset: i64) -> Self {
        if let Value::Object(fields) = &mut self.payload {
            fields.insert(OFFSET_FIELD.to_string(), Value::from(offset));
        }

        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row(payload: Value) -> EventRow {
        EventRow {
            session_id: Uuid::new_v4(),
            seq: 1,
            ts: Utc::now(),
            kind: "text".to_string(),
            payload,
        }
    }

    #[test]
    fn the_offset_field_is_internal() {
        assert_eq!(OFFSET_FIELD, "_offset");
        assert!(OFFSET_FIELD.starts_with(INTERNAL_FIELD_PREFIX));
    }

    #[test]
    fn a_public_payload_drops_underscore_fields() {
        let row = row(json!({ "text": "hello", "_offset": 512, "_internal": true }));
        assert_eq!(row.public_payload(), json!({ "text": "hello" }));
        // The stored payload is untouched: stripping happens on read.
        assert_eq!(row.payload["_offset"], json!(512));
    }

    #[test]
    fn a_public_payload_keeps_nested_underscore_fields() {
        let row = row(json!({
            "name": "Bash",
            "input": { "_raw": "ls" },
            "_offset": 12,
        }));
        assert_eq!(
            row.public_payload(),
            json!({ "name": "Bash", "input": { "_raw": "ls" } }),
        );
    }

    #[test]
    fn a_non_object_payload_survives_unchanged() {
        assert_eq!(row(json!([1, 2, 3])).public_payload(), json!([1, 2, 3]));
        assert_eq!(row(Value::Null).public_payload(), Value::Null);
    }

    #[test]
    fn a_new_event_is_observed_now_and_can_carry_its_offset() {
        let before = Utc::now();
        let event = NewEvent::now("text", json!({ "text": "hello" })).with_offset(2048);

        assert!(event.ts >= before);
        assert_eq!(event.kind, "text");
        assert_eq!(event.payload, json!({ "text": "hello", "_offset": 2048 }));
    }

    #[test]
    fn an_offset_on_a_non_object_payload_is_a_no_op() {
        let event = NewEvent::now("raw", json!("native line")).with_offset(7);
        assert_eq!(event.payload, json!("native line"));
    }

    #[test]
    fn a_row_round_trips_through_serde() {
        let row = row(json!({ "text": "hello" }));
        let encoded = serde_json::to_string(&row).unwrap();
        assert_eq!(serde_json::from_str::<EventRow>(&encoded).unwrap(), row);
    }
}
