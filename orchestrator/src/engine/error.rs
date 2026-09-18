//! [`EngineError`], the single failure type every [`ContainerEngine`]
//! operation returns.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" (Errors): the crate-wide
//! [`Error`] carries this as `#[from] EngineError` and delegates the HTTP
//! status to [`EngineError::status`]. Nothing outside `engine/` constructs
//! these; the bollard implementation translates the engine's own responses
//! into them so no caller has to know a `bollard` type.
//!
//! **Secrets.** A container's resolved secrets travel in
//! [`ContainerSpec::env`](super::ContainerSpec), so no message here is ever
//! built from a spec's environment: the identifier that failed and the
//! engine's own words, and nothing else (CLAUDE.md rule 3).

use axum::http::StatusCode;

// The crate convention (`CLAUDE.md`, "Backend conventions"); here for the doc
// links back to the crate-wide [`Error`] this widens into.
#[allow(unused_imports)]
use crate::prelude::*;

/// Anything the container engine refuses, cannot reach, or answers with a
/// failure.
///
/// The variants are the distinctions callers actually branch on. Everything
/// else the engine can say arrives as [`EngineError::Api`] with the status and
/// message it sent.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The engine socket is unreachable, refused the connection, or the
    /// connection broke mid-request. The string is the transport's own
    /// message.
    #[error("the container engine is unavailable: {0}")]
    Connection(String),
    /// The engine has no such container, image or network. The string names
    /// the object that was missing.
    ///
    /// This is the variant callers branch on: it is the difference between
    /// "the container is gone" and any other failure. Recovery reads a
    /// `NotFound` on inspect as a container that no longer exists and parks
    /// the session, where every other error leaves the session alone to be
    /// retried (`ARCHITECTURE.md`, "Durability and recovery"). The launcher
    /// reads it on `image_exists` as "pull it first", and removal treats it as
    /// success, because a container that is not there is already removed.
    #[error("the container engine has no such object: {0}")]
    NotFound(String),
    /// The engine refused because of the object's current state: a container
    /// name already in use, a container already started, a removal of
    /// something still running. The only variant that is a 409 rather than an
    /// internal fault.
    #[error("the container engine reports a conflict: {0}")]
    Conflict(String),
    /// Pulling an image failed. Kept apart from [`EngineError::Api`] because
    /// the launcher puts this message into `sessions.error` verbatim, where a
    /// missing tag or an unauthenticated registry is the operator's answer
    /// (`ARCHITECTURE.md`, "Session container specification").
    #[error("pulling image {image} failed: {message}")]
    ImagePull {
        /// The image reference that was being pulled.
        image: String,
        /// What the registry or the engine said.
        message: String,
    },
    /// The engine answered with an HTTP status the adapter does not translate
    /// into one of the variants above.
    #[error("the container engine answered {status}: {message}")]
    Api {
        /// The HTTP status the engine's API returned.
        status: u16,
        /// The message body the engine returned.
        message: String,
    },
    /// A local I/O failure while talking to the engine: reading an attached
    /// stream, writing to stdin, a broken socket.
    #[error("container engine I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// The operation is not available on this engine, or not on this
    /// implementation of the trait. The placeholder engine answers every real
    /// operation with it, and the bollard implementation uses it for a
    /// `HostConfig` field the running engine does not honour (ADR 0004).
    ///
    /// A missing capability and never a misconfiguration: a specification the
    /// builder refuses is [`EngineError::InvalidSpec`], so a launcher can tell
    /// "this engine cannot do that" from "this session is configured wrong".
    #[error("the container engine does not support this operation: {0}")]
    Unsupported(String),
    /// The container specification could not be built from the inputs it was
    /// given: a resolved secret named like one of
    /// [`RESERVED_ENV_NAMES`](super::spec::RESERVED_ENV_NAMES)
    /// (`ARCHITECTURE.md`, "Session container specification").
    ///
    /// The only 400 in this enum, and so the only variant whose message a
    /// caller is shown verbatim: it names the *name* that collides, which is
    /// something the operator chose and can change, and never a value
    /// (CLAUDE.md rule 3). Kept apart from [`EngineError::Unsupported`] so a
    /// launcher can tell a configuration error, which no retry and no other
    /// engine will fix, from a capability this engine lacks.
    #[error("the container specification is invalid: {0}")]
    InvalidSpec(String),
    /// The startup probe container did not prove what it has to prove: the
    /// file it wrote was not owned by the orchestrator's own uid, or was not
    /// writable (`ARCHITECTURE.md`, "Engine adapter", Startup probe). Fatal at
    /// startup, because every session would otherwise fail later in less
    /// obvious ways.
    #[error("the container engine probe failed: {0}")]
    Probe(String),
}

impl EngineError {
    /// The HTTP status this failure maps to.
    ///
    /// A state conflict the engine reports is a conflict for the caller too,
    /// and answers 409. A specification the builder refused is the caller's own
    /// input, and answers 400 with its message. Everything else is an internal
    /// fault: 500, logged once with its detail by [`Error`]'s `IntoResponse`,
    /// answered with the generic message.
    ///
    /// [`EngineError::NotFound`] is deliberately not a 404. A missing
    /// container is not a missing REST resource; the route that asked for it
    /// decides what to answer, and every caller that cares matches the variant
    /// rather than reading a status code back out.
    pub fn status(&self) -> StatusCode {
        match self {
            EngineError::Conflict(_) => StatusCode::CONFLICT,
            EngineError::InvalidSpec(_) => StatusCode::BAD_REQUEST,
            EngineError::Connection(_)
            | EngineError::NotFound(_)
            | EngineError::ImagePull { .. }
            | EngineError::Api { .. }
            | EngineError::Io(_)
            | EngineError::Unsupported(_)
            | EngineError::Probe(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_conflict_is_409_an_invalid_spec_is_400_and_everything_else_is_500() {
        assert_eq!(
            EngineError::Conflict("name is already in use".into()).status(),
            StatusCode::CONFLICT
        );

        // The caller's own input, so the 400 carries the message itself; the
        // name of the offending variable is the whole of it (rule 3).
        let invalid = EngineError::InvalidSpec(
            "secret name collides with a reserved variable: HOME".to_string(),
        );
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert!(invalid.to_string().contains("HOME"), "{invalid}");

        for error in [
            EngineError::Connection("connection refused".into()),
            EngineError::NotFound("mars-session-0".into()),
            EngineError::ImagePull {
                image: "mars-session-claude:dev".into(),
                message: "manifest unknown".into(),
            },
            EngineError::Api {
                status: 500,
                message: "engine failure".into(),
            },
            EngineError::Io(std::io::Error::other("broken pipe")),
            EngineError::Unsupported("placeholder engine".into()),
            EngineError::Probe("the probe file is owned by uid 100999".into()),
        ] {
            assert_eq!(
                error.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "unexpected status for {error}"
            );
        }
    }

    #[test]
    fn an_io_failure_converts_with_the_question_mark_operator() {
        fn fails() -> std::result::Result<(), EngineError> {
            Err(std::io::Error::other("the socket closed"))?;
            unreachable!("the conversion above always returns")
        }

        let error = fails().expect_err("the call fails");
        assert!(matches!(error, EngineError::Io(_)), "unexpected: {error:?}");
        assert!(error.to_string().contains("the socket closed"));
    }
}
