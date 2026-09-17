//! The one place the `tracing` subscriber is installed.
//!
//! `CLAUDE.md`, "Backend conventions": "Logging is `tracing` with structured
//! fields (`session_id = %id`), never string-formatted ids. Never log event
//! payloads at `info` or above." So `info!(session_id = %id, "session
//! started")`, never `info!("session {} started", id)` and never
//! `info!("{}", format!("session {id} started"))`: the id is a field, not part
//! of the message.
//!
//! Two things never reach a log line at `info` or above: event payloads (they
//! are large and they carry agent and user text) and secret values or tokens
//! of any kind (`CLAUDE.md` rule 3 — the two documented exceptions, ADR 0026
//! and ADR 0027, are elsewhere). A field is as public as the message around it.
//!
//! The filter string comes from `Config::rust_log` (`README.md`,
//! "Configuration": `RUST_LOG`, `info` by default); this module never reads the
//! environment itself. The `tower-http` `TraceLayer` is attached to the router
//! by the startup task, not here.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::prelude::warn;

/// The filter used when `RUST_LOG` is absent or unparseable (`README.md`,
/// "Configuration").
const DEFAULT_FILTER: &str = "info";

/// Why the tracing subscriber could not be installed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TelemetryError {
    /// A subscriber is already installed for this process. Installing one is
    /// global and happens once at startup; tests that call [`init_tracing`]
    /// repeatedly get this instead of a panic.
    #[error("a tracing subscriber is already installed for this process")]
    AlreadyInitialised,
}

/// Timestamps as RFC 3339 in UTC, to millisecond precision.
///
/// `chrono` rather than `tracing-subscriber`'s optional `time` feature, so no
/// crate or feature is added for a two-line formatter.
struct Rfc3339Utc;

impl FormatTime for Rfc3339Utc {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        write!(
            w,
            "{}",
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        )
    }
}

/// Install the process-wide `tracing` subscriber.
///
/// `filter` is an `env-filter` directive string, normally `Config::rust_log`.
/// An unparseable one is not fatal: the subscriber is installed with `info`
/// and a `warn!` names the offending string (a filter is not a secret), so a
/// typo in `RUST_LOG` never keeps the orchestrator from starting. Note that
/// `EnvFilter` reads a bare word such as `verbose` as a *target* directive
/// rather than rejecting it, so only genuinely unparseable strings (`=`,
/// `info=bogus`) take the fallback; either way startup continues.
///
/// Output goes to stderr, one compact line per event with an RFC 3339 UTC
/// timestamp, the level and the target. Never `.pretty()`: the lines are read
/// by `docker logs` and by log shippers, not by eye.
///
/// Installing a subscriber is a global, once-per-process action. A second call
/// returns [`TelemetryError::AlreadyInitialised`] rather than panicking, so
/// integration tests can call this from every test.
pub fn init_tracing(filter: &str) -> std::result::Result<(), TelemetryError> {
    let (env_filter, rejected) = match EnvFilter::try_new(filter) {
        Ok(env_filter) => (env_filter, None),
        Err(err) => (EnvFilter::new(DEFAULT_FILTER), Some(err.to_string())),
    };

    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_level(true)
        .with_timer(Rfc3339Utc);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .try_init()
        .map_err(|_| TelemetryError::AlreadyInitialised)?;

    // After installation, or the warning would have nowhere to go.
    if let Some(reason) = rejected {
        warn!(
            filter,
            error = %reason,
            default = DEFAULT_FILTER,
            "invalid log filter; falling back to the default"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test function, because installing a subscriber is global and the
    /// test binary shares a process: separate tests could not agree on which
    /// of them gets to be first.
    #[test]
    fn installs_once_and_then_reports_already_initialised() {
        // Whether this is the first installation in the process depends on the
        // rest of the binary, so accept either outcome here.
        let first = init_tracing("info");
        assert!(
            first.is_ok() || first == Err(TelemetryError::AlreadyInitialised),
            "unexpected first result: {first:?}"
        );

        assert_eq!(
            init_tracing("debug"),
            Err(TelemetryError::AlreadyInitialised),
            "a second installation must not panic"
        );

        // A bad filter is never fatal: it reaches the same installation step
        // as a good one, so in this process it too reports the subscriber that
        // is already there, not a parse failure.
        assert_eq!(
            init_tracing("not a filter"),
            Err(TelemetryError::AlreadyInitialised)
        );
        assert_eq!(
            init_tracing("info=bogus"),
            Err(TelemetryError::AlreadyInitialised)
        );
    }

    #[test]
    fn the_fallback_filter_is_the_documented_default() {
        // The fallback decision itself, independent of the global subscriber.
        // `EnvFilter` accepts a bare word as a target directive, so `verbose`
        // parses; `info=bogus` is what actually takes the fallback.
        assert!(EnvFilter::try_new("info=bogus").is_err());
        assert!(EnvFilter::try_new(DEFAULT_FILTER).is_ok());
        assert_eq!(DEFAULT_FILTER, "info");
    }
}
