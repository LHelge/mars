//! `session::rotate_token` against a real Postgres and a real data directory
//! (`CLAUDE.md`, "Testing expectations").
//!
//! The contract under test is ADR 0029's order: the replacement hash commits
//! before `mcp.json` is rewritten, and a session that may not be relaunched is
//! left with both its stored hash and its file exactly as they were. The path
//! shapes and the document's bytes are unit-tested in
//! `src/session/prepare.rs`; what needs a database is the state-scoped
//! `UPDATE` and the ordering around it.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::path::Path;

use mars_orchestrator::models::{NewSession, ProfileKind, SessionState, StateChange};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::SessionRepository;
use mars_orchestrator::session::{SessionDirs, initial_token, rotate_token, write_mcp_json};
use tempfile::TempDir;
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string the
/// seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// The `MCP_URL` the test writes; the default of `README.md`, "Configuration".
const MCP_URL: &str = "http://orchestrator:7001/mcp";

/// The rows a session needs to exist at all, seeded with unchecked statements
/// because only the foreign keys matter here.
struct Fixture {
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(pool: &PgPool) -> Fixture {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        .bind(FAKE_PASSWORD_HASH)
        .execute(pool)
        .await
        .expect("the user seeds");

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        .bind("https://git.example.test/mars.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, 'default', $3, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("localhost/mars-session:test")
    .execute(pool)
    .await
    .expect("the profile seeds");

    Fixture {
        project_id,
        profile_id,
    }
}

/// Insert a session the way a create route does: the initial token is generated
/// first and its hash travels in the row (ADR 0029).
async fn create_session(pool: &PgPool, fixture: &Fixture) -> (Uuid, String) {
    let token = initial_token();
    let hash = token.hash();
    let session = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        ProfileKind::Conversational,
        "main",
        hash.clone(),
    );

    let repository = SessionRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    (inserted.id, hash)
}

/// Drive the session to `state` through the documented lifecycle edges.
async fn move_to(pool: &PgPool, id: Uuid, states: &[SessionState]) {
    let repository = SessionRepository::new(pool);
    for state in states {
        let change = if *state == SessionState::Failed {
            StateChange::failed("test")
        } else {
            StateChange::plain()
        };
        let mut tx = pool.begin().await.expect("a transaction begins");
        repository
            .set_state(&mut tx, id, *state, &change)
            .await
            .expect("the transition is a documented edge");
        tx.commit().await.expect("the transaction commits");
    }
}

/// The stored hash, read with an unchecked query so no `.sqlx` entry is needed.
async fn stored_hash(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT mcp_token_hash FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the session is there")
}

/// A prepared session directory under a temporary data directory.
async fn prepared(data_dir: &Path, id: Uuid) -> SessionDirs {
    let dirs = SessionDirs::for_session(data_dir, data_dir, id);
    dirs.ensure().await.expect("the directories are created");
    dirs
}

#[tokio::test]
async fn rotation_on_a_parked_session_replaces_the_hash_and_rewrites_the_file() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let (id, initial_hash) = create_session(&pool, &fixture).await;
    move_to(&pool, id, &[SessionState::Running, SessionState::Parked]).await;

    let data = TempDir::new().expect("a temporary data directory");
    let dirs = prepared(data.path(), id).await;
    // The file the previous launch would have left behind.
    let previous = initial_token();
    write_mcp_json(&dirs, MCP_URL, &previous)
        .await
        .expect("the first config is written");

    let rotated = rotate_token(&pool, &dirs, MCP_URL, id)
        .await
        .expect("a parked session rotates");

    assert_eq!(stored_hash(&pool, id).await, rotated.hash());
    assert_ne!(rotated.hash(), initial_hash, "the hash did not change");

    let written = tokio::fs::read_to_string(dirs.mcp_json())
        .await
        .expect("the config is readable");
    assert!(
        written.contains(&format!("Bearer {}", rotated.expose())),
        "the file does not carry the new token"
    );
    assert!(
        !written.contains(previous.expose()),
        "the file still carries the replaced token"
    );
    assert!(
        !dirs.root().join("mcp.json.tmp").exists(),
        "a temporary file is left behind"
    );
}

#[tokio::test]
async fn rotation_on_a_creating_or_failed_session_is_allowed() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;

    // `creating`: the state a session is inserted in.
    let (creating, _) = create_session(&pool, &fixture).await;
    let data = TempDir::new().expect("a temporary data directory");
    let dirs = prepared(data.path(), creating).await;
    let token = rotate_token(&pool, &dirs, MCP_URL, creating)
        .await
        .expect("a creating session rotates");
    assert_eq!(stored_hash(&pool, creating).await, token.hash());

    // `failed`: the conversational retry path.
    let (failed, _) = create_session(&pool, &fixture).await;
    move_to(&pool, failed, &[SessionState::Failed]).await;
    let dirs = prepared(data.path(), failed).await;
    let token = rotate_token(&pool, &dirs, MCP_URL, failed)
        .await
        .expect("a failed session rotates");
    assert_eq!(stored_hash(&pool, failed).await, token.hash());
}

#[tokio::test]
async fn rotation_on_a_running_session_conflicts_and_changes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let (id, initial_hash) = create_session(&pool, &fixture).await;
    move_to(&pool, id, &[SessionState::Running]).await;

    let data = TempDir::new().expect("a temporary data directory");
    let dirs = prepared(data.path(), id).await;
    let live = initial_token();
    write_mcp_json(&dirs, MCP_URL, &live)
        .await
        .expect("the live config is written");
    let before = tokio::fs::read_to_string(dirs.mcp_json())
        .await
        .expect("the config is readable");

    let error = rotate_token(&pool, &dirs, MCP_URL, id)
        .await
        .expect_err("a running session does not rotate");

    match &error {
        Error::Conflict(message) => assert_eq!(message, "session is not relaunchable"),
        other => panic!("expected a conflict, got {other:?}"),
    }
    // 409 semantics, which is what the route would answer.
    assert_eq!(error.status(), axum::http::StatusCode::CONFLICT);

    assert_eq!(stored_hash(&pool, id).await, initial_hash, "the hash moved");
    let after = tokio::fs::read_to_string(dirs.mcp_json())
        .await
        .expect("the config is still there");
    assert_eq!(after, before, "the file was rewritten");
}

#[tokio::test]
async fn rotation_on_a_missing_session_conflicts() {
    let (_postgres, pool) = common::db::test_pool().await;
    let data = TempDir::new().expect("a temporary data directory");
    let id = Uuid::new_v4();
    let dirs = prepared(data.path(), id).await;

    let error = rotate_token(&pool, &dirs, MCP_URL, id)
        .await
        .expect_err("a session that is gone does not rotate");

    assert!(matches!(error, Error::Conflict(_)), "was {error:?}");
    assert!(
        !dirs.mcp_json().exists(),
        "a config was written for a session that does not exist"
    );
}

#[tokio::test]
async fn a_write_failure_after_the_hash_committed_leaves_the_previous_file_intact() {
    use std::os::unix::fs::PermissionsExt;

    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let (id, _) = create_session(&pool, &fixture).await;
    move_to(&pool, id, &[SessionState::Running, SessionState::Parked]).await;

    let data = TempDir::new().expect("a temporary data directory");
    let dirs = prepared(data.path(), id).await;
    let previous = initial_token();
    write_mcp_json(&dirs, MCP_URL, &previous)
        .await
        .expect("the first config is written");
    let before = tokio::fs::read_to_string(dirs.mcp_json())
        .await
        .expect("the config is readable");

    std::fs::set_permissions(dirs.root(), std::fs::Permissions::from_mode(0o500))
        .expect("the root is made read-only");
    let error = rotate_token(&pool, &dirs, MCP_URL, id).await.expect_err(
        "a rotation whose file cannot be written fails, so the caller starts no process",
    );
    std::fs::set_permissions(dirs.root(), std::fs::Permissions::from_mode(0o700))
        .expect("the mode is restored");

    assert!(matches!(error, Error::Internal(_)), "was {error:?}");
    // The hash committed before the write was attempted (ADR 0029); the file is
    // the one the previous launch left, unchanged.
    let after = tokio::fs::read_to_string(dirs.mcp_json())
        .await
        .expect("the previous config is still there");
    assert_eq!(after, before);
    assert!(
        after.contains(previous.expose()),
        "the previous token is gone from the file"
    );
}
