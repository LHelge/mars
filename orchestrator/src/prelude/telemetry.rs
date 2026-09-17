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
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::prelude::warn;

/// The filter used when `RUST_LOG` is absent, or set to something the
/// orchestrator refuses to run with (`README.md`, "Configuration").
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
/// A filter the orchestrator will not use is not fatal: the subscriber is
/// installed with `info` and a `warn!` names the offending string (a filter is
/// not a secret), so a typo in `RUST_LOG` never keeps the orchestrator from
/// starting. Two kinds take that fallback: genuinely unparseable strings
/// (`info=bogus`) and strings that parse but would silence the orchestrator,
/// because `EnvFilter` reads a bare word such as `verbose` as a *target*
/// directive rather than as a level. See [`select_filter`].
///
/// Output goes to stderr, one compact line per event with an RFC 3339 UTC
/// timestamp, the level and the target. Never `.pretty()`: the lines are read
/// by `docker logs` and by log shippers, not by eye.
///
/// Installing a subscriber is a global, once-per-process action. A second call
/// returns [`TelemetryError::AlreadyInitialised`] rather than panicking, so
/// integration tests can call this from every test.
pub fn init_tracing(filter: &str) -> std::result::Result<(), TelemetryError> {
    let (env_filter, rejected) = select_filter(filter);

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

/// Choose the filter to install, and say why the requested one was refused.
///
/// Pure, so the decision is testable without the global subscriber that
/// [`init_tracing`] installs. `None` means `filter` is used as written.
///
/// Beyond the strings `EnvFilter` itself rejects, this refuses a filter whose
/// every comma-separated directive is a bare word that is not a level name
/// (`trace`, `debug`, `info`, `warn`, `error`, `off`, case-insensitively, or
/// their numeric forms). `EnvFilter` reads such a word as a *target* directive
/// with no level, which enables nothing at all, so `RUST_LOG=verbose` would
/// silence the orchestrator instead of falling back to `info` as `README.md`,
/// "Configuration", implies. The check is deliberately narrow: one directive
/// carrying `=` or `[` span syntax, or one naming a level, is enough for the
/// whole filter to be taken as written (`sqlx=warn,info`, `mars_orchestrator`
/// alongside `debug`).
fn select_filter(filter: &str) -> (EnvFilter, Option<String>) {
    if let Some(word) = only_bare_non_level_words(filter) {
        return (
            EnvFilter::new(DEFAULT_FILTER),
            Some(format!("{word:?} is not a log level")),
        );
    }

    match EnvFilter::try_new(filter) {
        Ok(env_filter) => (env_filter, None),
        Err(err) => (EnvFilter::new(DEFAULT_FILTER), Some(err.to_string())),
    }
}

/// The first directive of `filter`, when every one of them is a bare word that
/// is not a level; `None` otherwise, including for an empty filter.
fn only_bare_non_level_words(filter: &str) -> Option<&str> {
    let directives: Vec<&str> = filter
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .collect();

    if directives.is_empty() || !directives.iter().all(|d| is_bare_non_level_word(d)) {
        return None;
    }

    directives.first().copied()
}

/// A directive with no target/level separator, no span syntax and no level
/// name: `EnvFilter` would take it for a target and enable nothing.
fn is_bare_non_level_word(directive: &str) -> bool {
    !directive.contains('=')
        && !directive.contains('[')
        && directive.parse::<LevelFilter>().is_err()
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
        assert!(EnvFilter::try_new("info=bogus").is_err());
        assert!(EnvFilter::try_new(DEFAULT_FILTER).is_ok());
        assert_eq!(DEFAULT_FILTER, "info");

        let (env_filter, reason) = select_filter("info=bogus");
        assert!(reason.is_some(), "an unparseable filter must be refused");
        assert_eq!(env_filter.to_string(), DEFAULT_FILTER);
    }

    #[test]
    fn a_bare_non_level_word_falls_back_to_info() {
        // `EnvFilter` would accept `verbose` as a target directive and enable
        // nothing, so `select_filter` refuses it instead of going silent.
        assert!(EnvFilter::try_new("verbose").is_ok());

        let (env_filter, reason) = select_filter("verbose");
        assert_eq!(
            reason.as_deref(),
            Some("\"verbose\" is not a log level"),
            "the reason names the offending word"
        );
        assert_eq!(env_filter.to_string(), DEFAULT_FILTER);

        // Every directive bare and none of them a level: still refused.
        assert!(select_filter("verbose,chatty").1.is_some());
    }

    #[test]
    fn legitimate_filters_are_used_as_written() {
        for filter in [
            "info",
            "debug",
            "INFO",
            "off",
            "sqlx=warn,info",
            "mars_orchestrator=debug",
            // One level word is enough to make the bare target meaningful.
            "mars_orchestrator,debug",
            // Span syntax is not a bare word.
            "[request]",
            // An empty filter is a deliberate "log nothing", not a typo.
            "",
        ] {
            let (_, reason) = select_filter(filter);
            assert_eq!(reason, None, "{filter:?} must be used as written");
        }
    }
}
