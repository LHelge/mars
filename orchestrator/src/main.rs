//! Mars orchestrator binary.
//!
//! Real startup — config, pool, migrations, listeners, recovery and the
//! background services — lands in the startup task; see `ARCHITECTURE.md`,
//! "Orchestrator internals".

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
