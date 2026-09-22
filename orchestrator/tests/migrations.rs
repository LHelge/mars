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

/// The load-bearing details of the `dispatcher_columns` migration
/// (`docs/data-model.md`, `agent_profiles`, `projects`, `sessions`; ADR 0042).
///
/// Nothing launches by itself yet, so no code path would notice any of these
/// going missing: the default that makes every pre-existing session a user
/// launch, the two lower bounds the models also check, and the partial index
/// the dispatcher's selection will read.
#[tokio::test]
async fn the_dispatcher_columns_carry_their_defaults_and_bounds() {
    let (_postgres, pool) = common::db::test_pool().await;

    // Every row written before this column existed is a user launch, and the
    // insert path that forgets it still stores one.
    let (column_default, is_nullable, data_type): (Option<String>, String, String) =
        sqlx::query_as(
            "SELECT column_default, is_nullable::text, udt_name::text \
             FROM information_schema.columns \
             WHERE table_schema = 'public' \
               AND table_name = 'sessions' \
               AND column_name = 'launch_source'",
        )
        .fetch_one(&pool)
        .await
        .expect("sessions.launch_source exists");
    assert_eq!(is_nullable, "NO");
    assert_eq!(data_type, "session_launch_source");
    assert_eq!(
        column_default.as_deref(),
        Some("'user'::session_launch_source"),
        "sessions.launch_source must default to 'user'"
    );

    // The three values of the enum, in order.
    let values: Vec<String> = sqlx::query_scalar(
        "SELECT e.enumlabel::text FROM pg_enum e \
         JOIN pg_type t ON t.oid = e.enumtypid \
         WHERE t.typname = 'session_launch_source' \
         ORDER BY e.enumsortorder",
    )
    .fetch_all(&pool)
    .await
    .expect("the enum catalog is readable");
    assert_eq!(values, ["user", "dispatcher", "schedule"]);

    // The caps the models refuse are refused by the database too, so a value
    // written any other way cannot become an unbounded launch budget.
    let profile_checks: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(c.oid) \
         FROM pg_constraint c \
         JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = 'public' AND t.relname = 'agent_profiles' AND c.contype = 'c'",
    )
    .fetch_all(&pool)
    .await
    .expect("the constraint catalog is readable");
    assert!(
        profile_checks
            .iter()
            .any(|def| def.contains("max_concurrent") && def.contains(">= 1")),
        "agent_profiles is missing the >= 1 CHECK on max_concurrent: {profile_checks:?}"
    );

    let project_checks: Vec<String> = sqlx::query_scalar(
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
        project_checks
            .iter()
            .any(|def| def.contains("max_concurrent_sessions") && def.contains(">= 1")),
        "projects is missing the >= 1 CHECK on max_concurrent_sessions: {project_checks:?}"
    );

    // Nullable on purpose: NULL is "no project cap", not a missing value.
    let cap_nullable: String = sqlx::query_scalar(
        "SELECT is_nullable::text FROM information_schema.columns \
         WHERE table_schema = 'public' \
           AND table_name = 'projects' \
           AND column_name = 'max_concurrent_sessions'",
    )
    .fetch_one(&pool)
    .await
    .expect("projects.max_concurrent_sessions exists");
    assert_eq!(cap_nullable, "YES");

    // The dispatcher's selection, partial and in its tie-break order.
    let (is_unique, predicate, definition): (bool, Option<String>, String) = sqlx::query_as(
        "SELECT i.indisunique, pg_get_expr(i.indpred, i.indrelid), pg_get_indexdef(i.indexrelid) \
         FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relname = 'agent_profiles_auto_launch_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("agent_profiles_auto_launch_idx exists");
    assert!(
        !is_unique,
        "a project may have several auto_launch profiles"
    );
    let predicate = predicate.expect("agent_profiles_auto_launch_idx must be partial");
    assert!(
        predicate.contains("auto_launch"),
        "the index must be predicated on auto_launch, not {predicate:?}"
    );
    assert!(
        definition.contains("project_id") && definition.contains("created_at"),
        "the index must order a project's profiles oldest first: {definition}"
    );
}

/// The load-bearing details of the `schedule_columns` migration
/// (`docs/data-model.md`, `agent_profiles`; ADR 0043).
///
/// Nothing fires yet, so nothing else would notice these going missing: the
/// three nullable columns, the `CHECK` that keeps an expression and its prompt
/// together whatever writes them, and the partial index the scheduler's scan
/// will read.
#[tokio::test]
async fn the_schedule_columns_are_nullable_and_paired() {
    let (_postgres, pool) = common::db::test_pool().await;

    let columns: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT column_name::text, is_nullable::text, udt_name::text \
         FROM information_schema.columns \
         WHERE table_schema = 'public' \
           AND table_name = 'agent_profiles' \
           AND column_name IN ('schedule_cron', 'schedule_prompt', 'last_scheduled_at') \
         ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .expect("the column catalog is readable");
    assert_eq!(
        columns,
        vec![
            (
                "last_scheduled_at".to_string(),
                "YES".to_string(),
                "timestamptz".to_string()
            ),
            (
                "schedule_cron".to_string(),
                "YES".to_string(),
                "text".to_string()
            ),
            (
                "schedule_prompt".to_string(),
                "YES".to_string(),
                "text".to_string()
            ),
        ],
        "the three schedule columns do not match the document"
    );

    // A schedule without its prompt is meaningless and a prompt without a
    // schedule is unread, whatever wrote the row.
    let checks: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(c.oid) \
         FROM pg_constraint c \
         JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = 'public' AND t.relname = 'agent_profiles' AND c.contype = 'c'",
    )
    .fetch_all(&pool)
    .await
    .expect("the constraint catalog is readable");
    assert!(
        checks
            .iter()
            .any(|def| def.contains("schedule_cron") && def.contains("schedule_prompt")),
        "agent_profiles is missing the schedule pair CHECK: {checks:?}"
    );

    // The scheduler's scan, partial: a schedule is the exception, not the rule.
    let (is_unique, predicate, definition): (bool, Option<String>, String) = sqlx::query_as(
        "SELECT i.indisunique, pg_get_expr(i.indpred, i.indrelid), pg_get_indexdef(i.indexrelid) \
         FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indexrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relname = 'agent_profiles_schedule_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("agent_profiles_schedule_idx exists");
    assert!(!is_unique, "a project may have several scheduled profiles");
    let predicate = predicate.expect("agent_profiles_schedule_idx must be partial");
    assert!(
        predicate.contains("schedule_cron"),
        "the index must be predicated on schedule_cron, not {predicate:?}"
    );
    assert!(
        definition.contains("project_id"),
        "the index must carry the project the caps are asked about: {definition}"
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

/// The load-bearing details of the `secrets` migration
/// (`docs/data-model.md`, "Secrets").
///
/// Two of them cannot be seen by reading the table definition back as a list
/// of columns, and both would fail silently rather than loudly. The unique key
/// is `NULLS NOT DISTINCT`, without which the NULL `scope_id` of every global
/// secret would make duplicates of the same global name legal; and `scope_id`
/// has no foreign key, because the scope decides which table it points at. The
/// `CHECK` tying `scope` to `scope_id` is asserted by inserting the rows it
/// exists to refuse.
#[tokio::test]
async fn secrets_schema_holds_the_documented_guarantees() {
    let (_postgres, pool) = common::db::test_pool().await;

    // The name matters as much as the constraint: the repository matches
    // Postgres's default name to turn a duplicate into a `Conflict`.
    let (nulls_not_distinct, definition): (bool, String) = sqlx::query_as(
        "SELECT i.indnullsnotdistinct, pg_get_constraintdef(c.oid) \
         FROM pg_index i \
         JOIN pg_class ic ON ic.oid = i.indexrelid \
         JOIN pg_namespace n ON n.oid = ic.relnamespace \
         JOIN pg_constraint c ON c.conindid = i.indexrelid \
         WHERE n.nspname = 'public' AND ic.relname = 'secrets_scope_scope_id_name_key'",
    )
    .fetch_one(&pool)
    .await
    .expect("the unique key carries Postgres's default name for (scope, scope_id, name)");
    assert!(
        nulls_not_distinct,
        "secrets_scope_scope_id_name_key must be declared NULLS NOT DISTINCT, or \
         global secrets could be duplicated: {definition}"
    );

    // No foreign key at all on `secrets`, other than `created_by`: `scope_id`
    // points at `users` or `projects` depending on `scope`, so the repository
    // validates it and a reaper deletes orphans.
    let foreign_keys: Vec<String> = sqlx::query_scalar(
        "SELECT constraint_name::text \
         FROM information_schema.table_constraints \
         WHERE constraint_schema = 'public' AND table_name = 'secrets' \
           AND constraint_type = 'FOREIGN KEY' \
         ORDER BY constraint_name",
    )
    .fetch_all(&pool)
    .await
    .expect("the constraint catalog is readable");
    assert_eq!(
        foreign_keys,
        vec!["secrets_created_by_fkey".to_string()],
        "secrets.scope_id must have no foreign key; only created_by has one"
    );

    // Rotation sweeps select by key version, so the index is what keeps them
    // from scanning the table.
    let (key_version_unique, key_version_predicate): (bool, Option<String>) =
        partial_index(&pool, "secrets_key_version_idx").await;
    assert!(
        !key_version_unique,
        "secrets_key_version_idx is a lookup index, not a uniqueness constraint"
    );
    assert_eq!(
        key_version_predicate, None,
        "secrets_key_version_idx covers every row, so rotation finds them all"
    );

    // The audit index is read newest-first for a secret, and the session index
    // skips the rows that record no session at all.
    let secret_uses_index: String = index_definition(&pool, "secret_uses_secret_idx").await;
    assert!(
        secret_uses_index.contains("at DESC"),
        "secret_uses_secret_idx must order `at` descending, not {secret_uses_index:?}"
    );
    let (_, session_predicate): (bool, Option<String>) =
        partial_index(&pool, "secret_uses_session_idx").await;
    let session_predicate =
        session_predicate.expect("secret_uses_session_idx must be a partial index");
    assert!(
        session_predicate.contains("session_id IS NOT NULL"),
        "secret_uses_session_idx must be predicated on session_id IS NOT NULL, not \
         {session_predicate:?}"
    );

    // `purpose` is `launch` or `git`, validated by the model: the document
    // lists no constraint, and adding one here would make a new purpose a
    // migration.
    let secret_use_checks: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(c.oid) \
         FROM pg_constraint c \
         JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = 'public' AND t.relname = 'secret_uses' AND c.contype = 'c'",
    )
    .fetch_all(&pool)
    .await
    .expect("the constraint catalog is readable");
    assert!(
        !secret_use_checks.iter().any(|def| def.contains("purpose")),
        "secret_uses.purpose must not be constrained by a CHECK: {secret_use_checks:?}"
    );

    // The scope/scope_id CHECK, from both sides. A global secret names no
    // target and a project secret always names one; the database is what makes
    // that true, so it is asserted by trying to break it.
    let global_with_target = insert_secret(&pool, "global", Some(FAKE_SCOPE_ID), "GLOBAL_WITH_ID")
        .await
        .expect_err("a global secret with a scope_id is refused");
    assert_check_violation(&global_with_target, "secrets");

    let project_without_target = insert_secret(&pool, "project", None, "PROJECT_WITHOUT_ID")
        .await
        .expect_err("a project secret without a scope_id is refused");
    assert_check_violation(&project_without_target, "secrets");

    // And the unique key, behaviourally: two global rows of the same name
    // differ only in a NULL `scope_id`, so this insert is what proves NULLS
    // NOT DISTINCT is in force rather than merely declared.
    insert_secret(&pool, "global", None, "ANTHROPIC_TOKEN")
        .await
        .expect("the first global secret of a name is accepted");
    let duplicate = insert_secret(&pool, "global", None, "ANTHROPIC_TOKEN")
        .await
        .expect_err("a second global secret of the same name is refused");
    assert_eq!(
        duplicate
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("secrets_scope_scope_id_name_key"),
        "the duplicate must be refused by the named unique key: {duplicate}"
    );
}

/// The one migration that changes rows rather than schema:
/// `strip_agent_credentials_from_profiles` (`docs/data-model.md`, "Migration
/// list for v1"; ADR 0036).
///
/// Asserting it means arranging a row it can find, which means stopping the
/// migrator just short of it, writing the profile the older code was allowed
/// to write, and then letting the rest run. What the migration owes is that
/// the credential names are gone and that nothing else about the array is:
/// the other entries and their order survive.
#[tokio::test]
async fn stripping_agent_credentials_leaves_the_other_profile_secrets_in_order() {
    let (_postgres, pool) = common::db::raw_pool().await;

    MIGRATOR
        .run_to(version_before(STRIP_AGENT_CREDENTIALS_VERSION), &pool)
        .await
        .expect("the migrations before the strip apply");

    // A profile of the shape the rule now refuses: the credential is listed
    // between two ordinary secrets.
    let project_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind("stripped")
        .bind("https://example.invalid/stripped.git")
        .execute(&pool)
        .await
        .expect("the project is inserted");
    let profile_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, secrets, partial_messages) \
         VALUES ($1, $2, $3, $4, $5, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("planner")
    .bind("localhost/mars-stub:latest")
    .bind(vec![
        "NPM_TOKEN".to_string(),
        "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
        "X".to_string(),
    ])
    .execute(&pool)
    .await
    .expect("the profile is inserted");

    MIGRATOR
        .run(&pool)
        .await
        .expect("the remaining migrations apply");

    let secrets: Vec<String> =
        sqlx::query_scalar("SELECT secrets FROM agent_profiles WHERE id = $1")
            .bind(profile_id)
            .fetch_one(&pool)
            .await
            .expect("the profile is readable");
    assert_eq!(
        secrets,
        vec!["NPM_TOKEN".to_string(), "X".to_string()],
        "the credential must be removed and the rest left in order"
    );
}

/// The version of the migration that strips agent credential names, as its
/// file name carries it.
const STRIP_AGENT_CREDENTIALS_VERSION: i64 = 20260921055132;

/// The version of the migration immediately before `version`, which is what
/// [`sqlx::migrate::Migrator::run_to`] takes to stop just short of it.
fn version_before(version: i64) -> i64 {
    MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .filter(|candidate| *candidate < version)
        .max()
        .unwrap_or_else(|| panic!("{version} is not the first migration"))
}

/// A syntactically valid UUID that belongs to no row; `scope_id` has no
/// foreign key, so nothing looks it up.
const FAKE_SCOPE_ID: &str = "00000000-0000-0000-0000-0000000000ff";

/// Insert a secret with obviously fake encrypted material (`CLAUDE.md`, rule
/// 3), returning whatever the database made of it.
async fn insert_secret(
    pool: &PgPool,
    scope: &str,
    scope_id: Option<&str>,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO secrets \
             (id, scope, scope_id, name, ciphertext, nonce, \
              data_key_wrapped, data_key_nonce, key_version) \
         VALUES (gen_random_uuid(), $1::secret_scope, $2::uuid, $3, $4, $5, $6, $7, 1)",
    )
    .bind(scope)
    .bind(scope_id)
    .bind(name)
    .bind(b"fake-ciphertext".as_slice())
    .bind(b"fake-nonce--".as_slice())
    .bind(b"fake-wrapped-data-key".as_slice())
    .bind(b"fake-wrap-no".as_slice())
    .execute(pool)
    .await
    .map(|_| ())
}

/// Assert that an insert was refused by a `CHECK` on the given table. The
/// constraint is unnamed in the migration, so its name is read back from the
/// catalog rather than spelled out here.
fn assert_check_violation(error: &sqlx::Error, table: &str) {
    let database_error = error
        .as_database_error()
        .unwrap_or_else(|| panic!("the insert failed in the database, not in sqlx: {error}"));
    let constraint = database_error
        .constraint()
        .unwrap_or_else(|| panic!("the failure names the constraint that refused it: {error}"));
    assert!(
        constraint.starts_with(table) && constraint.ends_with("_check"),
        "the insert must be refused by a CHECK on {table}, not by {constraint}"
    );
}

/// The `CREATE INDEX` statement Postgres reconstructs for an index.
async fn index_definition(pool: &PgPool, name: &str) -> String {
    sqlx::query_scalar(
        "SELECT indexdef::text FROM pg_indexes \
         WHERE schemaname = 'public' AND indexname = $1",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|_| panic!("{name} exists"))
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

/// The version of the `secrets` migration, which is the one the
/// `secrets_claude_credential_idx` migration sits directly on top of.
const SECRETS_MIGRATION_VERSION: i64 = 20260917071417;

/// A `secrets` row written by raw SQL, for the test that needs rows the write
/// rules would now refuse.
///
/// The encrypted columns are one obviously fake byte each (`CLAUDE.md`, rule
/// 3): nothing here ever decrypts them, and the migration under test reads
/// `scope`, `scope_id` and `name` only.
async fn insert_raw_secret(pool: &PgPool, scope: &str, name: &str) {
    sqlx::query(
        "INSERT INTO secrets (id, scope, scope_id, name, ciphertext, nonce, \
                              data_key_wrapped, data_key_nonce, key_version) \
         VALUES (gen_random_uuid(), $1::secret_scope, NULL, $2, \
                 '\\x00', '\\x00', '\\x00', '\\x00', 1)",
    )
    .bind(scope)
    .bind(name)
    .execute(pool)
    .await
    .expect("the raw secret inserts");
}

/// The literal names in `secrets_claude_credential_idx` are the Claude
/// backend's `credential_names` (ADR 0036; `docs/data-model.md`, `secrets`).
///
/// An index predicate cannot call into Rust, so the two names are written out
/// in the migration and this is what stops them drifting from the adapter that
/// really owns them: a backend that renames or adds a credential fails here
/// until its migration follows.
#[tokio::test]
async fn the_credential_index_predicate_is_the_claude_backends_credential_names() {
    let (_postgres, pool) = common::db::test_pool().await;

    let (unique, predicate) = partial_index(&pool, "secrets_claude_credential_idx").await;
    assert!(unique, "the credential index must be unique");
    let predicate = predicate.expect("the credential index is partial");

    // The predicate comes back as `name = ANY (ARRAY['A'::text, 'B'::text])`,
    // so the quoted halves are the literals.
    let mut in_the_index: Vec<String> = predicate
        .split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    in_the_index.sort();

    let mut declared: Vec<String> =
        mars_orchestrator::agent::backend_for(mars_orchestrator::models::AgentBackend::Claude)
            .credential_names()
            .iter()
            .map(|name| name.as_str().to_string())
            .collect();
    declared.sort();

    assert_eq!(
        in_the_index, declared,
        "the index predicate and AgentBackend::credential_names have drifted: {predicate}"
    );

    // The scope pair is what is unique, and a `global` row's NULL `scope_id`
    // has to collide with another `global` row's.
    let definition = index_definition(&pool, "secrets_claude_credential_idx").await;
    assert!(
        definition.contains("NULLS NOT DISTINCT"),
        "the credential index must treat NULL scope_ids as equal: {definition}"
    );
}

/// A database that already holds both credentials at one scope fails the
/// migration with a message naming the scope and what to do about it.
///
/// Without the `DO` block the operator would see a bare unique violation on an
/// index name, with no way to tell which of their scopes is the problem
/// (`docs/data-model.md`, "Migration list for v1").
#[tokio::test]
async fn the_credential_index_migration_names_the_scope_that_holds_both() {
    let (_postgres, pool) = common::db::raw_pool().await;

    MIGRATOR.run(&pool).await.expect("every migration applies");
    MIGRATOR
        .undo(&pool, SECRETS_MIGRATION_VERSION)
        .await
        .expect("the credential index reverts");

    // Two credentials at one scope: impossible once the index exists, which is
    // exactly the database this migration has to refuse readably.
    insert_raw_secret(&pool, "global", "ANTHROPIC_API_KEY").await;
    insert_raw_secret(&pool, "global", "CLAUDE_CODE_OAUTH_TOKEN").await;

    let error = MIGRATOR
        .run(&pool)
        .await
        .expect_err("the migration refuses a scope holding both credentials");
    let message = error.to_string();

    assert!(message.contains("global"), "{message}");
    assert!(message.contains("ANTHROPIC_API_KEY"), "{message}");
    assert!(message.contains("CLAUDE_CODE_OAUTH_TOKEN"), "{message}");
    assert!(message.contains("delete"), "{message}");
}
