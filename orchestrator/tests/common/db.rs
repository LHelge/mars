//! One throw-away Postgres per test *run*, one throw-away database per test.
//!
//! Every repository test, the migration round-trip test and
//! `TestApp::spawn()` build on this. The first process of a run to ask for a
//! pool starts `postgres:18` in a container and applies the crate's
//! migrations once, to a database called `mars_template`; every test then gets
//! `CREATE DATABASE test_<uuid> TEMPLATE mars_template` on that same server,
//! which costs milliseconds instead of the seconds a container start costs.
//! Tests in one binary still run in parallel and still share nothing: a
//! database of one's own isolates rows, sequences, `LISTEN`/`NOTIFY` channels
//! and advisory locks alike, because a PostgreSQL advisory lock tag carries
//! the database oid.
//!
//! # One server for the whole run
//!
//! The run is `cargo nextest run`, which puts every test in a process of its
//! own (ADR 0037). A server per process would therefore be a server per test,
//! so the processes of a run share one.
//!
//! What they share is a state directory under `CARGO_TARGET_TMPDIR`, which is
//! inside the build directory and so belongs to this checkout and this build
//! alone. It holds a `lock` file every process takes `flock(LOCK_EX)` on
//! before it reads or writes anything else, a `server` file naming the URL,
//! the container id and a fingerprint of the embedded migrations, and one
//! empty file per live process under `pids/`. A process finding a `server`
//! whose fingerprint is its own and whose URL answers joins it; anything else
//! — a different fingerprint, a container someone removed by hand — is torn
//! down and replaced, so a stale server can never serve an old schema. The
//! last process to leave removes the container and the `server` file.
//!
//! A process killed outright leaves its `pids/` entry behind. The next
//! process prunes it: a pid no process answers to is not holding anything.
//! Reusing a pid is the one way that can go wrong, and all it costs is a
//! container that outlives its run.
//!
//! Starting a container needs a reachable engine socket. The harness does not
//! configure one: it relies on the default socket resolution of
//! `testcontainers`, which reads `DOCKER_HOST` and then the usual Docker and
//! Podman socket paths. CI runners provide Docker; on a development machine
//! export `DOCKER_HOST` at the Podman socket first (`README.md`,
//! "Development").
//!
//! # Runtimes
//!
//! Each `#[tokio::test]` has a runtime of its own that is dropped when the
//! test ends, so nothing that outlives a test may be bound to one: a `PgPool`
//! whose reaper task died with its runtime hands out connections registered
//! with a terminated reactor. So the shared state here holds no pool. The
//! container is started on a leaked multi-threaded runtime of its own, from a
//! scoped thread outside any runtime (`Runtime::block_on` panics inside
//! another runtime), and what the `OnceLock` keeps afterwards is a URL and a
//! handle. Every database is created and dropped over a single connection
//! opened and closed on whichever runtime asked for it.
//!
//! # Reaping the container
//!
//! `testcontainers` 0.27 removes a container in `ContainerAsync::drop`, and
//! the Rust crate ships no Ryuk reaper to fall back on (unlike the Java and Go
//! libraries, so `TESTCONTAINERS_RYUK_*` does nothing here). Drop is no use
//! either: the container guard lives in a `static` for the whole process, and
//! the test harness ends with `std::process::exit`, which runs no
//! destructors. The container is therefore removed from a `libc::atexit`
//! hook — `atexit` handlers *do* run on `process::exit` — which opens a
//! `bollard` client on the same socket `testcontainers` used and forces the
//! container away. The hook is the same one that removes this process from
//! `pids/`, and it only removes the container when no other process of the
//! run is left. A run that is killed outright (`SIGKILL`) leaks its
//! container, which is the one case no in-process mechanism can cover.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::fd::AsRawFd;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bollard::query_parameters::RemoveContainerOptionsBuilder;
use bollard::{API_DEFAULT_VERSION, Docker};
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::runtime::Handle;
use uuid::Uuid;

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

/// The server arguments the container runs with.
///
/// `fsync=off` is what the `testcontainers` Postgres module passes by default
/// and is repeated here because `ImageExt::with_cmd` replaces the image's
/// command rather than extending it. `max_connections` is raised from the
/// built-in 100 because one server now carries every test of the *run* at
/// once: sixteen test processes at [`DEFAULT_MAX_CONNECTIONS`] plus a listener
/// and a maintenance connection each fit inside 100 only just, and the
/// fan-out tests ask for more.
const SERVER_ARGS: [&str; 4] = ["-c", "fsync=off", "-c", "max_connections=300"];

/// The database the migrations are applied to once and every test database is
/// cloned from.
const TEMPLATE_DATABASE: &str = "mars_template";

/// The maintenance database `CREATE DATABASE` and `DROP DATABASE` are issued
/// against. The image creates it; nothing in Mars uses it.
const ADMIN_DATABASE: &str = "postgres";

/// How many times a `CREATE DATABASE` is tried before the test fails.
///
/// The clones used to be serialised by a process-wide advisory lock, on the
/// argument that a harness that flakes is worse than one that is a few hundred
/// milliseconds slower. Under nextest that lock spans the whole run, and a
/// suite measured with it is 9 % slower than one measured without
/// (ADR 0037). PostgreSQL takes only a `ShareLock` on the source database, so
/// concurrent clones of one template are legal; what they can meet is a
/// transient "source database is being accessed by other users" when a
/// connection to the template has not finished closing, and a retry is the
/// answer to that rather than a queue.
const CREATE_DATABASE_ATTEMPTS: u32 = 4;

/// How long the first retry waits; the second waits twice that, and so on.
const CREATE_DATABASE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

/// How long the apparently last process of a run waits before it believes it
/// (see [`leave`]).
///
/// Long enough to cover the runner starting the next test, short enough that
/// the end of a run is not noticeably later for it.
const REMOVAL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// Names a Postgres server the suite should use instead of starting its own:
/// `postgres://user:password@host:port`, with no database name.
///
/// For a run whose engine cannot give `testcontainers` a published port — the
/// Engine workflow's rootless Podman 4 answers `PortNotExposed` — and which
/// therefore brings a server of its own (a CI service container). The server
/// must be this run's alone: the template database is dropped and rebuilt on
/// it, and no container is started or removed. Unset, which is every local run
/// and Orchestrator CI, the suite starts its own server as the module
/// documentation describes.
pub const EXTERNAL_SERVER_ENV: &str = "MARS_TEST_POSTGRES_URL";

/// How long the removal hook's single engine request may take, in seconds.
const REMOVE_TIMEOUT_SECS: u64 = 30;

/// The process-wide server every test database is created on.
struct SharedPostgres {
    /// `postgres://postgres:postgres@host:port`, with no database name: a
    /// test database's URL is this plus `/<name>`.
    ///
    /// The credentials are the image's defaults and exist only for the
    /// lifetime of the container; nothing real is spelled out here (rule 3).
    base_url: String,
    /// The leaked runtime the container was started on, which is also where a
    /// [`TestDatabase`] drop schedules its `DROP DATABASE`: a test's own
    /// runtime is already shutting down by then and would never poll it.
    runtime: Handle,
    /// Never read; it keeps the container guard alive for the whole process so
    /// that `testcontainers` cannot remove the server under a running test.
    /// Removal is the `atexit` hook's job (see the module documentation).
    /// `None` on a server the run brought itself ([`EXTERNAL_SERVER_ENV`]).
    _container: Option<ContainerAsync<Postgres>>,
}

static SHARED: OnceLock<SharedPostgres> = OnceLock::new();

/// The shared server, started on first use.
///
/// Synchronous and blocking on purpose: `OnceLock::get_or_init` makes the
/// first caller pay the container start and every other caller wait for it,
/// which is exactly the arrangement wanted, and each `#[tokio::test]` has a
/// thread of its own to block.
fn shared() -> &'static SharedPostgres {
    SHARED.get_or_init(|| {
        // A scoped thread, because `Runtime::block_on` panics when it is
        // called from inside another runtime and this function is called from
        // a test's own runtime.
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = Box::leak(Box::new(
                        tokio::runtime::Builder::new_multi_thread()
                            .worker_threads(2)
                            .enable_all()
                            .build()
                            .expect("a runtime for the shared server"),
                    ));

                    runtime.block_on(boot(runtime.handle().clone()))
                })
                .join()
                .expect("the shared server starts")
        })
    })
}

/// Join the run's server, starting it if this process is the first.
///
/// Everything here happens under the state directory's lock, so a sibling
/// process either waits for the server this one starts or is the one that
/// started it (see the module documentation).
async fn boot(runtime: Handle) -> SharedPostgres {
    let state = state_directory();
    let lock = StateLock::acquire(&state);
    prune_pids(&state);

    let fingerprint = migrations_fingerprint();
    let mut joined = None;
    let mut container = None;

    if let Some(server) = read_server(&state) {
        if server.fingerprint == fingerprint && answers(&server.base_url).await {
            joined = Some(server.base_url);
        } else {
            // A server of another build, or one whose container is gone: take
            // it away rather than clone a schema this binary was not compiled
            // against.
            discard_server(&state, &server).await;
        }
    }

    let base_url = match joined {
        Some(base_url) => base_url,
        None => {
            let (base_url, started) = start_server().await;
            create_template(&base_url).await;
            write_server(
                &state,
                &Server {
                    fingerprint,
                    base_url: base_url.clone(),
                    container: started.as_ref().map(|started| started.id().to_string()),
                },
            );
            container = started;

            base_url
        }
    };

    register_departure(&state);
    drop(lock);

    SharedPostgres {
        base_url,
        runtime,
        _container: container,
    }
}

/// Start the server this run's databases live on.
///
/// Either the one [`EXTERNAL_SERVER_ENV`] names, which the run brought itself
/// and nothing here removes, or a container of this run's own.
async fn start_server() -> (String, Option<ContainerAsync<Postgres>>) {
    if let Some(base_url) = external_server() {
        drop_template(&base_url).await;

        return (base_url, None);
    }

    // `trust` because `scram-sha-256` is the single most expensive thing a
    // test does: the SCRAM exchange is 4096 rounds of PBKDF2 at each end, and
    // an unoptimised test binary spends about 55 ms of its own CPU on every
    // connection it opens — three of them per `TestApp::spawn`. The password
    // in the URL below is then ignored. Nothing real is reachable: the server
    // is this run's container, on a port published to the loopback address,
    // and it is removed when the run ends (`CLAUDE.md`, rule 3, is about
    // credentials that mean something somewhere).
    let container = Postgres::default()
        .with_tag(POSTGRES_TAG)
        .with_cmd(SERVER_ARGS)
        .with_env_var("POSTGRES_HOST_AUTH_METHOD", "trust")
        .start()
        .await
        .expect("postgres starts");

    let base_url = format!(
        "postgres://postgres:postgres@{}:{}",
        container
            .get_host()
            .await
            .expect("the container has a host"),
        container
            .get_host_port_ipv4(5432)
            .await
            .expect("the container publishes 5432"),
    );

    (base_url, Some(container))
}

/// Whether a server recorded earlier is still there to be used.
async fn answers(base_url: &str) -> bool {
    let Ok(connection) = PgConnection::connect(&format!("{base_url}/{ADMIN_DATABASE}")).await
    else {
        return false;
    };
    let _ = connection.close().await;

    true
}

/// The server [`EXTERNAL_SERVER_ENV`] names, without a trailing slash.
fn external_server() -> Option<String> {
    let url = std::env::var(EXTERNAL_SERVER_ENV).ok()?;
    let url = url.trim().trim_end_matches('/');

    (!url.is_empty()).then(|| url.to_string())
}

/// Remove a template an earlier run left on an external server, so that this
/// run's migrations are the ones its tests clone.
async fn drop_template(base_url: &str) {
    let mut admin = admin_connection(base_url).await;
    execute(
        &mut admin,
        format!(
            "UPDATE pg_database SET datistemplate = false WHERE datname = '{TEMPLATE_DATABASE}'"
        ),
    )
    .await
    .expect("the old template database is unmarked");
    execute(
        &mut admin,
        format!("DROP DATABASE IF EXISTS \"{TEMPLATE_DATABASE}\""),
    )
    .await
    .expect("the old template database is dropped");
    let _ = admin.close().await;
}

/// Create [`TEMPLATE_DATABASE`], migrate it and mark it a template.
///
/// The pool that runs the migrations is closed before the flag goes up:
/// `CREATE DATABASE ... TEMPLATE` refuses a source database another session is
/// connected to, so a lingering idle connection here would fail every test in
/// the binary.
async fn create_template(base_url: &str) {
    let mut admin = admin_connection(base_url).await;
    execute(
        &mut admin,
        format!("CREATE DATABASE \"{TEMPLATE_DATABASE}\""),
    )
    .await
    .expect("the template database is created");
    let _ = admin.close().await;

    let template = PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{base_url}/{TEMPLATE_DATABASE}"))
        .await
        .expect("the template pool connects");
    MIGRATOR
        .run(&template)
        .await
        .expect("the migrations apply to the template");
    template.close().await;

    let mut admin = admin_connection(base_url).await;
    execute(
        &mut admin,
        format!(
            "UPDATE pg_database SET datistemplate = true WHERE datname = '{TEMPLATE_DATABASE}'"
        ),
    )
    .await
    .expect("the template database is marked as one");
    let _ = admin.close().await;
}

/// A single connection to [`ADMIN_DATABASE`] on the shared server.
async fn admin_connection(base_url: &str) -> PgConnection {
    PgConnection::connect(&format!("{base_url}/{ADMIN_DATABASE}"))
        .await
        .expect("a connection to the maintenance database")
}

/// Run one statement that takes no arguments.
///
/// `sqlx::raw_sql` rather than `query!`: these are DDL statements built at run
/// time, and `CREATE DATABASE` is not something `.sqlx/` should ever have to
/// carry a cached plan for (`CLAUDE.md`, "Backend conventions"). The only
/// thing ever interpolated into one is a name this module generated itself,
/// which is the assertion `sqlx::AssertSqlSafe` asks for.
async fn execute(connection: &mut PgConnection, statement: String) -> sqlx::Result<()> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
        .execute(connection)
        .await
        .map(|_| ())
}

/// Create a database on the shared server, optionally cloned from a template.
async fn create_database(template: Option<&str>) -> TestDatabase {
    let shared = shared();
    // 32 hex digits and the prefix: 37 bytes, well inside the 63-byte limit
    // on an identifier.
    let name = format!("test_{}", Uuid::new_v4().simple());

    let mut admin = admin_connection(&shared.base_url).await;
    let statement = match template {
        Some(template) => format!("CREATE DATABASE \"{name}\" TEMPLATE \"{template}\""),
        None => format!("CREATE DATABASE \"{name}\""),
    };

    let mut created = execute(&mut admin, statement.clone()).await;
    for attempt in 1..CREATE_DATABASE_ATTEMPTS {
        if created.is_ok() {
            break;
        }
        tokio::time::sleep(CREATE_DATABASE_BACKOFF * attempt).await;
        created = execute(&mut admin, statement.clone()).await;
    }

    let _ = admin.close().await;
    created.expect("the test database is created");

    TestDatabase {
        url: format!("{}/{name}", shared.base_url),
        name,
    }
}

/// One test's database on the shared server.
///
/// What the pool helpers return beside the pool, and what `TestApp` holds: a
/// test has to keep it for as long as it uses the pool, because dropping it
/// takes the database away.
#[derive(Debug)]
pub struct TestDatabase {
    name: String,
    url: String,
}

impl TestDatabase {
    /// The database name, `test_` plus a UUID without hyphens.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The URL of this database, for a `DATABASE_URL` or a second pool.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for TestDatabase {
    /// Drop the database on the shared runtime, best effort.
    ///
    /// `DROP DATABASE` is asynchronous work and `Drop` is not, and the test's
    /// own runtime is being torn down at this point, so the statement is
    /// spawned on the shared server's runtime instead, which lives as long as
    /// the process. `WITH (FORCE)` is what makes it succeed while the test's
    /// pool is still closing its connections.
    ///
    /// Nothing waits for it and nothing asserts on it: a test that panicked
    /// before its guard dropped, or a process that exited before the task ran,
    /// leaves a database behind, and every leftover goes away with the
    /// container.
    fn drop(&mut self) {
        let Some(shared) = SHARED.get() else {
            return;
        };

        let base_url = shared.base_url.clone();
        let name = std::mem::take(&mut self.name);
        shared.runtime.spawn(async move {
            let Ok(mut admin) =
                PgConnection::connect(&format!("{base_url}/{ADMIN_DATABASE}")).await
            else {
                return;
            };
            let _ = execute(
                &mut admin,
                format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"),
            )
            .await;
            let _ = admin.close().await;
        });
    }
}

/// A pool on a fresh database with **no** migrations applied, so the caller
/// controls the migration state itself. The round-trip test is the reason this
/// exists; ordinary tests want [`test_pool`].
pub async fn raw_pool() -> (TestDatabase, PgPool) {
    raw_pool_with(DEFAULT_MAX_CONNECTIONS).await
}

/// [`raw_pool`] with an explicit pool size.
pub async fn raw_pool_with(max_connections: u32) -> (TestDatabase, PgPool) {
    let database = create_database(None).await;
    let pool = connect(&database, max_connections).await;

    (database, pool)
}

/// A pool on a fresh database with the crate's migrations applied.
///
/// The migrations are not run here: the database is a clone of the migrated
/// [`TEMPLATE_DATABASE`], `_sqlx_migrations` rows and all, so `MIGRATOR.run()`
/// on it would find nothing to do.
pub async fn test_pool() -> (TestDatabase, PgPool) {
    test_pool_with(DEFAULT_MAX_CONNECTIONS).await
}

/// [`test_pool`] with an explicit pool size.
pub async fn test_pool_with(max_connections: u32) -> (TestDatabase, PgPool) {
    let database = create_database(Some(TEMPLATE_DATABASE)).await;
    let pool = connect(&database, max_connections).await;

    (database, pool)
}

/// The pool every helper above hands back.
async fn connect(database: &TestDatabase, max_connections: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database.url())
        .await
        .expect("the pool connects")
}

/// The run's state directory, inside this build directory's `tmp/`.
///
/// `CARGO_TARGET_TMPDIR` is set for integration test binaries only, which is
/// all of this module's callers, and it follows `CARGO_TARGET_DIR`: two
/// worktrees building into two directories therefore share no server, which is
/// what a parallel agent needs (`CLAUDE.md`, "Git workflow").
fn state_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mars-test-postgres")
}

/// What the `server` file records: the one line each of fingerprint, URL and
/// container id, in that order, with `-` for a server the run did not start.
struct Server {
    fingerprint: String,
    base_url: String,
    container: Option<String>,
}

/// The exclusive `flock` on the state directory, held for as long as the guard
/// is.
///
/// `flock` and not a lock file's existence: the kernel releases it however the
/// process ends, so a panicking or killed process cannot wedge a run.
struct StateLock(File);

impl StateLock {
    /// Take the lock, waiting for whoever holds it.
    fn acquire(state: &Path) -> StateLock {
        fs::create_dir_all(state).expect("the test state directory exists");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state.join("lock"))
            .expect("the test state lock file opens");

        // SAFETY: `flock` takes a descriptor this process owns and keeps open
        // for the lifetime of the guard, and `LOCK_EX` is a documented
        // operation. It blocks, which is the intent.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        assert_eq!(locked, 0, "the test state lock is taken");

        StateLock(file)
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        // SAFETY: the same descriptor, still open. Closing it would release
        // the lock too; this only makes the release the visible thing.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// The recorded server, if the file is there and well formed.
///
/// Anything unreadable is treated as no record at all: the worst that follows
/// is one more container start.
fn read_server(state: &Path) -> Option<Server> {
    let recorded = fs::read_to_string(state.join("server")).ok()?;
    let mut lines = recorded.lines();
    let fingerprint = lines.next()?.to_string();
    let base_url = lines.next()?.to_string();
    let container = lines.next()?;

    Some(Server {
        fingerprint,
        base_url,
        container: (container != "-").then(|| container.to_string()),
    })
}

/// Record the server for the processes that follow.
fn write_server(state: &Path, server: &Server) {
    let mut file = File::create(state.join("server")).expect("the server record is created");
    writeln!(
        file,
        "{}\n{}\n{}",
        server.fingerprint,
        server.base_url,
        server.container.as_deref().unwrap_or("-"),
    )
    .expect("the server record is written");
}

/// Take a recorded server away: its container, if it owned one, and its
/// record.
async fn discard_server(state: &Path, server: &Server) {
    if let Some(id) = &server.container {
        remove_container(&engine_socket(), id).await;
    }
    let _ = fs::remove_file(state.join("server"));
}

/// A fingerprint of the migrations this binary embeds.
///
/// Two binaries of one build agree on it and a binary built after a migration
/// changed does not, which is the whole question a reused template has to
/// answer.
fn migrations_fingerprint() -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    for migration in MIGRATOR.iter() {
        digest.update(migration.version.to_le_bytes());
        digest.update(&*migration.checksum);
    }

    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// This process's entry under `pids/`, which the exit hook removes.
static DEPARTURE: OnceLock<PathBuf> = OnceLock::new();

/// Announce this process as a user of the server and arrange for it to
/// announce its departure.
///
/// Called with the state lock held.
fn register_departure(state: &Path) {
    let pids = state.join("pids");
    fs::create_dir_all(&pids).expect("the pid directory exists");
    let entry = pids.join(std::process::id().to_string());
    File::create(&entry).expect("this process registers itself");

    if DEPARTURE.set(entry).is_err() {
        // `boot` runs once behind a `OnceLock`, so a second registration would
        // be a bug rather than a race.
        return;
    }

    // SAFETY: `atexit` asks for an `extern "C" fn()` that neither takes nor
    // returns anything and does not unwind. `leave` is one, and it catches its
    // own panics. The `OnceLock` above makes this the only registration.
    let registered = unsafe { libc::atexit(leave) };
    assert_eq!(registered, 0, "the departure hook registers");
}

/// Drop the entries of processes that are no longer running.
///
/// Called with the state lock held.
fn prune_pids(state: &Path) {
    let Ok(entries) = fs::read_dir(state.join("pids")) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            let _ = fs::remove_file(entry.path());
            continue;
        };
        if !running(pid) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Whether a process with this pid exists.
fn running(pid: i32) -> bool {
    // SAFETY: signal 0 is the documented existence check and sends nothing.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }

    // `EPERM` is another user's process, which is alive; only `ESRCH` says the
    // pid is free.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// How many processes of the run are still registered.
///
/// Called with the state lock held, after [`prune_pids`].
fn remaining(state: &Path) -> usize {
    fs::read_dir(state.join("pids"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

/// Leave the run, and take the server with it if nobody else is using it.
///
/// Registered with `atexit`, so it runs on the `std::process::exit` the test
/// harness finishes with.
extern "C" fn leave() {
    let Some(entry) = DEPARTURE.get() else {
        return;
    };
    let Some(state) = entry.parent().and_then(Path::parent) else {
        return;
    };

    let left = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let lock = StateLock::acquire(state);
        let _ = fs::remove_file(entry);
        prune_pids(state);
        let last = remaining(state) == 0;
        drop(lock);

        if !last {
            return Ok(());
        }

        // Being the last process of the moment is not being the last process
        // of the run: the runner starts the next test as soon as this one's
        // slot frees, and a process that tore the server down in that gap
        // would make the next one start a container of its own — three or
        // four times over a suite, each of them seconds the whole run waits
        // for behind the lock. So the last one out waits for the gap to close
        // before it believes itself, with the lock released so that whoever
        // arrives can take it.
        std::thread::sleep(REMOVAL_GRACE);

        let lock = StateLock::acquire(state);
        prune_pids(state);
        if remaining(state) == 0
            && let Some(server) = read_server(state)
        {
            // A current-thread runtime, built and finished inside this
            // handler: no thread is spawned and nothing outlives the call,
            // which is as much as an exit handler should ask of the process.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime.block_on(discard_server(state, &server));
        }

        drop(lock);

        Ok(())
    }));

    match left {
        Ok(Ok(())) => {}
        Ok(Err(error)) => eprintln!("could not clean up the test Postgres server: {error}"),
        Err(_) => eprintln!("cleaning up the test Postgres server panicked"),
    }
}

/// Force a container away through the engine socket `testcontainers` used.
async fn remove_container(engine: &str, id: &str) {
    let removed = async {
        let engine = connect_engine(engine).map_err(|error| error.to_string())?;
        let options = RemoveContainerOptionsBuilder::default()
            .force(true)
            .v(true)
            .build();

        engine
            .remove_container(id, Some(options))
            .await
            .map_err(|error| error.to_string())
    }
    .await;

    if let Err(error) = removed {
        eprintln!("could not remove the test Postgres container {id}: {error}");
    }
}

/// A `bollard` client on the socket the container was started through, opened
/// the same way `BollardEngine::connect` and `tests/common/engine.rs` open
/// one.
fn connect_engine(host: &str) -> Result<Docker, bollard::errors::Error> {
    match host.strip_prefix("unix://") {
        Some(path) => Docker::connect_with_socket(path, REMOVE_TIMEOUT_SECS, API_DEFAULT_VERSION),
        None => Docker::connect_with_http(host, REMOVE_TIMEOUT_SECS, API_DEFAULT_VERSION),
    }
}

/// The engine socket `testcontainers` started the container on.
///
/// Its own resolution is `pub(crate)`, so the order is repeated here:
/// `DOCKER_HOST`, then the default Docker socket, then the rootless
/// alternatives, then the default again as a last guess. A
/// `~/.testcontainers.properties` override is the one case not covered, and
/// the `properties-config` feature that reads it is off.
fn engine_socket() -> String {
    const DEFAULT: &str = "unix:///var/run/docker.sock";

    if let Some(host) = std::env::var("DOCKER_HOST")
        .ok()
        .filter(|host| !host.trim().is_empty())
    {
        return host;
    }

    let candidates = [
        Some(PathBuf::from("/var/run/docker.sock")),
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join(".docker/run/docker.sock")),
        std::env::var_os("HOME").map(|dir| PathBuf::from(dir).join(".docker/run/docker.sock")),
        std::env::var_os("HOME").map(|dir| PathBuf::from(dir).join(".docker/desktop/docker.sock")),
    ];

    candidates
        .into_iter()
        .flatten()
        .find(|path| path.exists())
        .map(|path| format!("unix://{}", path.display()))
        .unwrap_or_else(|| DEFAULT.to_string())
}
