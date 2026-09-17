//! A throw-away Postgres per test.
//!
//! Every repository test, the migration round-trip test and (once it exists)
//! `TestApp::spawn()` build on this: start `postgres:18` in a container,
//! connect a pool to it and apply the crate's migrations. Tests in one binary
//! run in parallel and each one gets its own container, so there is no shared
//! state to isolate and no cleanup between tests.
//!
//! Starting a container needs a reachable engine socket. The harness does not
//! configure one: it relies on the default socket resolution of
//! `testcontainers`, which reads `DOCKER_HOST` and then the usual Docker and
//! Podman socket paths. CI runners provide Docker; on a development machine
//! export `DOCKER_HOST` at the Podman socket first (`README.md`,
//! "Development").
//!
//! The container guard is returned, not dropped: it stops Postgres when it
//! goes out of scope, so a test has to hold it for as long as it uses the
//! pool.

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

/// The same `Migrator` `main.rs` runs at startup, embedded at compile time
/// from the crate's `migrations/` directory, so tests never drift from the
/// schema the binary applies.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The pinned server version. Bumping it is a deliberate change: the schema in
/// `docs/data-model.md` is written against this major version.
pub const POSTGRES_TAG: &str = "18";

/// Enough connections for a test that talks to the database one query at a
/// time. Tests that fan out (concurrent event appends) ask for more through
/// the `_with` variants.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 5;

/// Start a Postgres container and wait until it accepts connections.
pub async fn start_postgres() -> ContainerAsync<Postgres> {
    Postgres::default()
        .with_tag(POSTGRES_TAG)
        .start()
        .await
        .expect("postgres starts")
}

/// The URL of a started container.
///
/// The credentials are the image's defaults and exist only for the lifetime of
/// the container; nothing real is spelled out here (rule 3).
pub async fn connection_string(postgres: &ContainerAsync<Postgres>) -> String {
    format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        postgres.get_host().await.expect("the container has a host"),
        postgres
            .get_host_port_ipv4(5432)
            .await
            .expect("the container publishes 5432"),
    )
}

/// A pool on a fresh container with **no** migrations applied, so the caller
/// controls the migration state itself. The round-trip test is the reason this
/// exists; ordinary tests want [`test_pool`].
pub async fn raw_pool() -> (ContainerAsync<Postgres>, PgPool) {
    raw_pool_with(DEFAULT_MAX_CONNECTIONS).await
}

/// [`raw_pool`] with an explicit pool size.
pub async fn raw_pool_with(max_connections: u32) -> (ContainerAsync<Postgres>, PgPool) {
    let postgres = start_postgres().await;
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(&connection_string(&postgres).await)
        .await
        .expect("the pool connects");

    (postgres, pool)
}

/// A pool on a fresh container with the crate's migrations applied.
pub async fn test_pool() -> (ContainerAsync<Postgres>, PgPool) {
    test_pool_with(DEFAULT_MAX_CONNECTIONS).await
}

/// [`test_pool`] with an explicit pool size.
pub async fn test_pool_with(max_connections: u32) -> (ContainerAsync<Postgres>, PgPool) {
    let (postgres, pool) = raw_pool_with(max_connections).await;
    MIGRATOR.run(&pool).await.expect("migrations apply");

    (postgres, pool)
}
