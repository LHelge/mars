//! Every migration applies and every `.down.sql` fully reverses its
//! `.up.sql` (`docs/data-model.md`, "Migration list for v1").
//!
//! The round trip is up, down to nothing, up again. What makes it worth
//! running is the assertion in the middle: after reverting everything, the
//! `public` schema holds nothing but sqlx's own bookkeeping table and no enum
//! type is left behind. A `.down.sql` that forgets a table, or an enum created
//! by an `.up.sql` and never dropped, fails here rather than on the next
//! developer's database.
//!
//! The second test covers the one thing a migration puts in a fresh database
//! rather than in its schema: the seeded administrator.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use common::db::MIGRATOR;
use sqlx::PgPool;

#[tokio::test]
async fn every_migration_applies_and_fully_reverses() {
    let (_postgres, pool) = common::db::raw_pool().await;

    // Counted from the directory rather than from the `Migrator`, so a
    // migration that the macro embedded but Postgres never recorded is visible
    // as a mismatch.
    let expected = reversible_migration_count();

    MIGRATOR.run(&pool).await.expect("every migration applies");
    assert_eq!(
        applied_count(&pool).await,
        expected,
        "the number of recorded migrations does not match migrations/"
    );

    // 0 is the version to keep, so everything is reverted. A migration without
    // a `.down.sql` is not silently skipped: `reversible_migration_count`
    // above already refused it.
    MIGRATOR
        .undo(&pool, 0)
        .await
        .expect("every migration reverses");
    assert_eq!(
        applied_count(&pool).await,
        0,
        "reverting everything left migrations recorded as applied"
    );

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text \
         FROM information_schema.tables \
         WHERE table_schema = 'public' \
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .expect("the table catalog is readable");
    assert_eq!(
        tables,
        vec!["_sqlx_migrations".to_string()],
        "a .down.sql left tables behind"
    );

    let enums: Vec<String> = sqlx::query_scalar(
        "SELECT t.typname::text \
         FROM pg_type t \
         JOIN pg_namespace n ON n.oid = t.typnamespace \
         WHERE t.typtype = 'e' AND n.nspname = 'public' \
         ORDER BY t.typname",
    )
    .fetch_all(&pool)
    .await
    .expect("the type catalog is readable");
    assert!(
        enums.is_empty(),
        "a .down.sql left enum types behind: {enums:?}"
    );

    MIGRATOR
        .run(&pool)
        .await
        .expect("every migration applies again on a reverted database");
    assert_eq!(
        applied_count(&pool).await,
        expected,
        "re-applying after a full revert recorded a different set of migrations"
    );
}

/// The `users` migration seeds the bootstrap administrator with the fixed
/// default password documented in `README.md`, "Start" (ADR 0024).
///
/// The hash is a literal in the migration, so nothing verifies it at
/// deployment time: a mistyped PHC string would lock the operator out of a
/// fresh instance with no way in. Verifying it here is the only check there
/// is.
#[tokio::test]
async fn seeded_admin_is_present_with_changeme_hash() {
    let (_postgres, pool) = common::db::test_pool().await;

    let (username, email, password_hash, must_change_password, admin): (
        String,
        String,
        String,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT username, email, password_hash, must_change_password, admin \
         FROM users \
         WHERE id = '00000000-0000-0000-0000-000000000001'",
    )
    .fetch_one(&pool)
    .await
    .expect("the seeded administrator exists at the documented fixed id");

    assert_eq!(username, "admin");
    assert_eq!(email, "admin@localhost");
    assert!(
        must_change_password,
        "the seeded administrator must be forced to change the default password"
    );
    assert!(admin, "the seeded administrator is an admin");

    let parsed = PasswordHash::new(&password_hash).expect("the seeded hash is a valid PHC string");
    assert_eq!(
        parsed.algorithm.as_str(),
        "argon2id",
        "the seeded hash is Argon2id"
    );
    Argon2::default()
        .verify_password(b"changeme", &parsed)
        .expect("the seeded hash is the hash of the documented default password");

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("the users table is readable");
    assert_eq!(count, 1, "the migration seeds exactly one user");
}

/// The load-bearing details of the `projects` and `sessions` migrations
/// (`docs/data-model.md`, "Projects and profiles" and "Sessions and events").
///
/// Columns and types are checked by the code that queries them; what is
/// asserted here is the handful of schema details that nothing else would
/// notice going missing, because the database is the only thing enforcing
/// them: the readiness `CHECK` on `projects`, the `RESTRICT` that refuses to
/// delete a profile with sessions, the partial unique index behind "exactly
/// one default profile per project", the deliberate absence of a default on
/// `agent_profiles.partial_messages`, and the equally deliberate absence of
/// any index on `events` beyond its primary key.
///
/// One container serves all of them; they only read the catalogs.
#[tokio::test]
async fn projects_and_sessions_schema_holds_the_documented_guarantees() {
    let (_postgres, pool) = common::db::test_pool().await;

    // A project is only ready once its integration head resolves, so
    // `status = 'ready'` with a null `default_branch` is rejected by the
    // database rather than by whichever code path forgot to look.
    let checks: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(c.oid) \
         FROM pg_constraint c \
         JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = 'public' AND t.relname = 'projects' AND c.contype = 'c'",
    )
    .fetch_all(&pool)
    .await
    .expect("the constraint catalog is readable");
    assert!(
        checks
            .iter()
            .any(|def| def.contains("default_branch") && def.contains("ready")),
        "projects is missing the `status <> 'ready' OR default_branch IS NOT NULL` CHECK: {checks:?}"
    );
    assert!(
        checks
            .iter()
            .any(|def| def.contains("max_attempts") && def.contains("20")),
        "projects is missing the 1-20 CHECK on max_attempts: {checks:?}"
    );

    // Deleting a profile that still has sessions is refused; the sessions must
    // not be orphaned or cascaded away with it.
    let delete_rule: String = sqlx::query_scalar(
        "SELECT delete_rule::text \
         FROM information_schema.referential_constraints \
         WHERE constraint_schema = 'public' AND constraint_name = 'sessions_profile_id_fkey'",
    )
    .fetch_one(&pool)
    .await
    .expect("sessions.profile_id has a foreign key under Postgres's default name");
    assert_eq!(
        delete_rule, "RESTRICT",
        "sessions.profile_id must be ON DELETE RESTRICT"
    );

    // Exactly one default profile per project, enforced by a partial unique
    // index rather than by the repository reading before it writes.
    let (is_unique, predicate): (bool, Option<String>) = sqlx::query_as(
        "SELECT i.indisunique, pg_get_expr(i.indpred, i.indrelid) \
         FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relname = 'agent_profiles_one_default_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("agent_profiles_one_default_idx exists");
    assert!(is_unique, "agent_profiles_one_default_idx must be UNIQUE");
    let predicate = predicate.expect("agent_profiles_one_default_idx must be partial");
    assert!(
        predicate.contains("is_default"),
        "agent_profiles_one_default_idx must be predicated on is_default, not {predicate:?}"
    );

    // No column default by design: the model fills `partial_messages` from
    // `kind`, and an insert that forgets it has to fail rather than silently
    // pick a streaming mode.
    let (column_default, is_nullable): (Option<String>, String) = sqlx::query_as(
        "SELECT column_default, is_nullable::text \
         FROM information_schema.columns \
         WHERE table_schema = 'public' \
           AND table_name = 'agent_profiles' \
           AND column_name = 'partial_messages'",
    )
    .fetch_one(&pool)
    .await
    .expect("agent_profiles.partial_messages exists");
    assert_eq!(
        column_default, None,
        "agent_profiles.partial_messages must have no default"
    );
    assert_eq!(
        is_nullable, "NO",
        "agent_profiles.partial_messages must be NOT NULL"
    );

    // Every read of `events` is by session and a `seq` range, which the
    // primary key serves. A second index would only cost write throughput on
    // the hottest insert path there is.
    let event_indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes \
         WHERE schemaname = 'public' AND tablename = 'events' \
         ORDER BY indexname",
    )
    .fetch_all(&pool)
    .await
    .expect("the index catalog is readable");
    assert_eq!(
        event_indexes,
        vec!["events_pkey".to_string()],
        "events must carry its primary key and no other index"
    );

    // `task_id` and `handoff_id` belong to the `tasks` migration, which owns
    // the tables they reference, but they are columns of `sessions` and both
    // are optional: a session may be launched without a task at all.
    let session_columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT column_name::text, is_nullable::text FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = 'sessions' \
           AND column_name IN ('task_id', 'handoff_id') \
         ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .expect("the column catalog is readable");
    assert_eq!(
        session_columns,
        vec![
            ("handoff_id".to_string(), "YES".to_string()),
            ("task_id".to_string(), "YES".to_string()),
        ],
        "sessions must carry the nullable task_id and handoff_id the tasks migration adds"
    );

    // Deleting a task or a hand-off leaves its sessions in place; they are the
    // record of work that actually ran.
    let session_delete_rules: Vec<(String, String)> = sqlx::query_as(
        "SELECT constraint_name::text, delete_rule::text \
         FROM information_schema.referential_constraints \
         WHERE constraint_schema = 'public' \
           AND constraint_name IN ('sessions_task_id_fkey', 'sessions_handoff_id_fkey') \
         ORDER BY constraint_name",
    )
    .fetch_all(&pool)
    .await
    .expect("the referential constraint catalog is readable");
    assert_eq!(
        session_delete_rules,
        vec![
            (
                "sessions_handoff_id_fkey".to_string(),
                "SET NULL".to_string()
            ),
            ("sessions_task_id_fkey".to_string(), "SET NULL".to_string()),
        ],
        "sessions.task_id and sessions.handoff_id must be ON DELETE SET NULL"
    );

    let session_indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes \
         WHERE schemaname = 'public' AND tablename = 'sessions' \
         ORDER BY indexname",
    )
    .fetch_all(&pool)
    .await
    .expect("the index catalog is readable");
    assert_eq!(
        session_indexes,
        vec![
            "sessions_container_id_idx".to_string(),
            "sessions_mcp_token_hash_key".to_string(),
            "sessions_pkey".to_string(),
            "sessions_project_created_idx".to_string(),
            "sessions_state_idx".to_string(),
            "sessions_task_idx".to_string(),
        ],
        "the sessions indexes do not match the document"
    );
}

/// The load-bearing details of the `tasks` migration (`docs/data-model.md`,
/// "Tasks").
///
/// As above, the point is the handful of guarantees only the database can
/// make: the deliberate *absence* of a foreign key on `task_events.task_id`,
/// the `RESTRICT` that keeps a state with tasks in it from being deleted, and
/// the two partial indexes whose predicates are what make the claimable query
/// and "at most one human state" work at all. Everything else is exercised by
/// the repositories that query it.
#[tokio::test]
async fn tasks_schema_holds_the_documented_guarantees() {
    let (_postgres, pool) = common::db::test_pool().await;

    // Event history keeps the task's original UUID after the task is deleted,
    // so a foreign key here would either erase history or block deletion. The
    // primary key must be the only constraint of its kind on the table.
    let task_event_constraints: Vec<(String, String)> = sqlx::query_as(
        "SELECT constraint_name::text, constraint_type::text \
         FROM information_schema.table_constraints \
         WHERE constraint_schema = 'public' AND table_name = 'task_events' \
           AND constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY') \
         ORDER BY constraint_name",
    )
    .fetch_all(&pool)
    .await
    .expect("the constraint catalog is readable");
    assert_eq!(
        task_event_constraints,
        vec![
            ("task_events_pkey".to_string(), "PRIMARY KEY".to_string()),
            (
                "task_events_project_id_fkey".to_string(),
                "FOREIGN KEY".to_string()
            ),
        ],
        "task_events must have only the (project_id, seq) primary key and the project \
         foreign key; task_id deliberately has none"
    );

    let task_id_keys: Vec<String> = sqlx::query_scalar(
        "SELECT k.constraint_name::text \
         FROM information_schema.key_column_usage k \
         JOIN information_schema.table_constraints c \
           ON c.constraint_schema = k.constraint_schema \
          AND c.constraint_name = k.constraint_name \
         WHERE k.table_schema = 'public' AND k.table_name = 'task_events' \
           AND k.column_name = 'task_id' AND c.constraint_type = 'FOREIGN KEY'",
    )
    .fetch_all(&pool)
    .await
    .expect("the key column catalog is readable");
    assert!(
        task_id_keys.is_empty(),
        "task_events.task_id must have no foreign key, but has {task_id_keys:?}"
    );

    // A state that still holds tasks cannot be deleted; the repository refuses
    // first, but this is what makes the refusal true.
    let state_delete_rule: String = sqlx::query_scalar(
        "SELECT delete_rule::text \
         FROM information_schema.referential_constraints \
         WHERE constraint_schema = 'public' AND constraint_name = 'tasks_state_id_fkey'",
    )
    .fetch_one(&pool)
    .await
    .expect("tasks.state_id has a foreign key under Postgres's default name");
    assert_eq!(
        state_delete_rule, "RESTRICT",
        "tasks.state_id must be ON DELETE RESTRICT"
    );

    // The `ready` tool reads only claimable tasks, so the index must exclude
    // the blocked and the already-held ones rather than merely order them.
    let (claimable_unique, claimable_predicate): (bool, Option<String>) =
        partial_index(&pool, "tasks_claimable_idx").await;
    assert!(
        !claimable_unique,
        "tasks_claimable_idx is an ordering index, not a uniqueness constraint"
    );
    let claimable_predicate =
        claimable_predicate.expect("tasks_claimable_idx must be a partial index");
    assert!(
        claimable_predicate.contains("NOT blocked")
            && claimable_predicate.contains("lease_holder_session_id IS NULL"),
        "tasks_claimable_idx must be predicated on NOT blocked AND \
         lease_holder_session_id IS NULL, not {claimable_predicate:?}"
    );

    // At most one human state per project, enforced by a partial unique index
    // rather than by the repository reading before it writes.
    let (human_unique, human_predicate): (bool, Option<String>) =
        partial_index(&pool, "task_states_one_human_idx").await;
    assert!(human_unique, "task_states_one_human_idx must be UNIQUE");
    let human_predicate =
        human_predicate.expect("task_states_one_human_idx must be a partial index");
    assert!(
        human_predicate.contains("human"),
        "task_states_one_human_idx must be predicated on kind = 'human', not \
         {human_predicate:?}"
    );
}

/// Whether an index is unique, and its partial predicate if it has one.
async fn partial_index(pool: &PgPool, name: &str) -> (bool, Option<String>) {
    sqlx::query_as(
        "SELECT i.indisunique, pg_get_expr(i.indpred, i.indrelid) \
         FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relname = $1",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|_| panic!("{name} exists"))
}

/// How many migrations sqlx has recorded as applied.
async fn applied_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .expect("the migrations table is readable")
}

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

/// The number of `.up.sql` files in `migrations/`, after checking that the
/// directory holds reversible pairs and nothing else.
///
/// Migrations are created with `sqlx migrate add -r <name>` (`CLAUDE.md`,
/// "Backend conventions"), which writes `<version>_<name>.up.sql` and
/// `<version>_<name>.down.sql`. A bare `<version>_<name>.sql` is a migration
/// created without `-r` and cannot be reverted, so it fails here explicitly
/// instead of being quietly skipped by `Migrator::undo`.
fn reversible_migration_count() -> i64 {
    let dir = migrations_dir();
    let entries = std::fs::read_dir(&dir).expect("migrations/ is readable");

    let mut ups = BTreeSet::new();
    let mut downs = BTreeSet::new();
    let mut irreversible = Vec::new();

    for entry in entries {
        let name = entry
            .expect("a directory entry is readable")
            .file_name()
            .to_string_lossy()
            .into_owned();

        if let Some(stem) = name.strip_suffix(".up.sql") {
            ups.insert(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".down.sql") {
            downs.insert(stem.to_string());
        } else if name.ends_with(".sql") {
            irreversible.push(name);
        }
    }

    assert!(
        irreversible.is_empty(),
        "migrations/ holds migrations created without `sqlx migrate add -r`, \
         which cannot be reverted: {irreversible:?}"
    );

    let missing_down: Vec<_> = ups.difference(&downs).collect();
    assert!(
        missing_down.is_empty(),
        "migrations without a .down.sql: {missing_down:?}"
    );

    let missing_up: Vec<_> = downs.difference(&ups).collect();
    assert!(
        missing_up.is_empty(),
        ".down.sql files without a .up.sql: {missing_up:?}"
    );

    i64::try_from(ups.len()).expect("the migration count fits in an i64")
}
