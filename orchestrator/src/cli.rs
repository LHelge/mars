//! The binary's subcommands.
//!
//! Private to the binary — declared from `main.rs`, not from `lib.rs` — because
//! nothing in the library or the tests calls a subcommand as a function: the
//! contract is the process, its stdout line and its exit code, and the
//! integration test spawns the built binary to assert exactly that.
//!
//! There is one subcommand today and no argument parser behind it. `main.rs`
//! matches on the arguments itself; `clap` is deliberately not a dependency of
//! this crate, and the crate table in `ARCHITECTURE.md`, "Orchestrator
//! internals", is what would have to change first.

use mars_orchestrator::prelude::*;
use mars_orchestrator::secrets;

use crate::{Bootstrap, EXIT_FAILURE};

/// Rows were left behind the newest master key, so the operator (or the script
/// that called this) has to run the sweep again before dropping the old key
/// from `SECRETS_MASTER_KEYS`. Distinct from [`EXIT_FAILURE`] because nothing
/// went wrong: the sweep ran and did not finish.
pub const EXIT_INCOMPLETE: i32 = 2;

/// Re-wrap every secret behind the newest master key, print the report and
/// exit.
///
/// The operator's alternative to waiting for the hourly cron job after adding a
/// key (`ARCHITECTURE.md`, "Secrets", Rotation; `README.md`, "Operating
/// notes"). It is safe to run while the orchestrator serves: `rewrap_outdated`
/// writes one row at a time under `WHERE id = $1 AND key_version = $2`, so a
/// concurrent write to a row wins and the row is counted as skipped rather than
/// overwritten.
///
/// Nothing else is started: no listener, no session registry, no recovery and
/// no cron. The bootstrap this is handed has already connected, migrated and
/// verified the keyring against the stored rows.
pub async fn rotate_secrets(bootstrap: Bootstrap) -> ! {
    let Bootstrap { pool, keyring, .. } = bootstrap;

    let report = match secrets::rewrap_outdated(&pool, &keyring).await {
        Ok(report) => report,
        Err(err) => {
            error!(error = %err, "the rotation sweep failed");
            pool.close().await;
            std::process::exit(EXIT_FAILURE);
        }
    };

    info!(
        rewrapped = report.rewrapped,
        skipped = report.skipped,
        remaining = report.remaining,
        "rotation sweep finished"
    );

    // The contract a script reads, on stdout, beside the `info!` above: the
    // logs go to stderr (`prelude::telemetry`), so this line is alone there
    // whatever `RUST_LOG` says. Counts only — no name, no scope, no value
    // (rule 3).
    println!(
        "rewrapped={} skipped={} remaining={}",
        report.rewrapped, report.skipped, report.remaining
    );

    pool.close().await;

    if report.remaining > 0 {
        warn!(
            remaining = report.remaining,
            "rows are still behind the newest master key; run rotate-secrets again and keep the \
             older keys configured until this reaches zero"
        );
        std::process::exit(EXIT_INCOMPLETE);
    }

    std::process::exit(0);
}
