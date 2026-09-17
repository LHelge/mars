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
