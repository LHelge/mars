//! The startup probe: a short-lived container run with the same `HostConfig` a
//! session would get, proving that a file it writes under `DATA_DIR` comes
//! back owned by the orchestrator's own uid.
//!
//! Empty until the task that writes it. It is what verifies that Podman
//! honours `keep-id` and that the bind mounts and uid layout are sane on
//! either engine (`ARCHITECTURE.md`, "Engine adapter", Startup probe;
//! ADR 0004). A failure is [`EngineError::Probe`](super::EngineError::Probe)
//! and is fatal at startup.
