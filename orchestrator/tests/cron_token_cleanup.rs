//! The hourly token cleanup (`ARCHITECTURE.md`, "Background jobs": "Delete
//! expired refresh tokens, reset tokens, unaccepted invites, and secrets whose
//! scope row no longer exists").
//!
//! One sweep over a database seeded with a row on each side of every one of
//! the four predicates, because the job's whole contract is what it leaves
//! alone: a revoked-but-unexpired refresh token, an unspent reset link, a live
//! invite, an accepted invite that expired long ago, and every `global`
//! secret. The single `now` the job is given is what puts each row on its
//! side, which is why the job takes its clock from the caller at all.
//!
//! The sweep is also the only thing that frees an email address again:
//! `user_invites_open_email_idx` does not know about `expires_at`, so the
//! re-invite at the end is the visible half of the deletion
//! (`docs/data-model.md`, `user_invites`).
//!
//! No value here is or resembles a credential (`CLAUDE.md`, rule 3): the token
//! hashes are obviously fake strings and the secret envelopes are fake bytes.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use chrono::{TimeDelta, Utc};
use common::TestApp;
use mars_orchestrator::cron::JobReport;
use mars_orchestrator::models::{
    Email, NewProject, NewSecret, ScopeRef, SecretName, SecretUsePurpose,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{
    PasswordResetTokenRepository, ProjectRepository, RefreshTokenRepository, SecretRepository,
    UserInviteRepository, UserRepository,
};
use mars_orchestrator::secrets::{SealedSecret, SecretIdentity, WrappedKey};
use uuid::Uuid;

/// Not a real remote: the fixture the project tests use (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// Obviously fake stand-ins for the SHA-256 hex of a token (rule 3).
const LIVE_REFRESH: &str = "fake-live-refresh-token-hash";
const EXPIRED_REFRESH: &str = "fake-expired-refresh-token-hash";
const REVOKED_REFRESH: &str = "fake-revoked-refresh-token-hash";
const LIVE_RESET: &str = "fake-live-reset-token-hash";
const EXPIRED_RESET: &str = "fake-expired-reset-token-hash";
const LIVE_INVITE: &str = "fake-live-invite-token-hash";
const EXPIRED_INVITE: &str = "fake-expired-invite-token-hash";
const ACCEPTED_INVITE: &str = "fake-accepted-invite-token-hash";
const REPLACEMENT_INVITE: &str = "fake-replacement-invite-token-hash";

/// The address whose only invite expires, and which the re-invite at the end
/// claims again.
const EXPIRED_INVITE_EMAIL: &str = "grace@example.com";

/// Not key material: an obviously fake envelope for a row no test opens
/// (rule 3).
fn fake_secret(scope: ScopeRef, raw_name: &str, tag: &str) -> NewSecret {
    let name = SecretName::parse(raw_name).expect("the test secret name is valid");

    NewSecret::new(SealedSecret {
        identity: SecretIdentity::new(&scope, &name),
        ciphertext: format!("fake-ciphertext-{tag}").into_bytes(),
        nonce: format!("fake-nonce-{tag}").into_bytes(),
        wrapped: WrappedKey {
            wrapped: format!("fake-wrapped-data-key-{tag}").into_bytes(),
            nonce: format!("fake-wrap-nonce-{tag}").into_bytes(),
            version: 1,
        },
    })
}

/// Insert a secret and one `secret_uses` row for it, both committed.
async fn seed_secret(pool: &PgPool, secret: &NewSecret) -> Uuid {
    let repository = SecretRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, secret)
        .await
        .expect("the secret inserts");
    repository
        .insert_use(&mut tx, inserted.id, None, None, SecretUsePurpose::Launch)
        .await
        .expect("the use records");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// How many `secret_uses` rows exist. No interface answers this once the
/// secret they belong to is gone, which is the cascade this asserts.
async fn count_secret_uses(pool: &PgPool) -> i64 {
    sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!" FROM secret_uses"#)
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

/// Whether the invite row still exists in any state. `list_open` hides an
/// accepted invite, so only a row-level read says it survived.
async fn invite_exists(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM user_invites WHERE id = $1) AS "exists!""#,
        id,
    )
    .fetch_one(pool)
    .await
    .expect("the existence check runs")
}

#[tokio::test]
async fn the_sweep_deletes_the_expired_and_orphaned_rows_only() {
    let app = TestApp::spawn().await;
    let pool = &app.pool;
    let now = Utc::now();
    let hour = TimeDelta::try_hours(1).expect("an hour is a valid delta");
    let second = TimeDelta::try_seconds(1).expect("a second is a valid delta");

    let refresh_tokens = RefreshTokenRepository::new(pool);
    let reset_tokens = PasswordResetTokenRepository::new(pool);
    let invites = UserInviteRepository::new(pool);
    let secrets = SecretRepository::new(pool);

    let ada = app
        .insert_user("ada", "ada@example.com", false, false)
        .await;

    // One refresh token on each side of `now`, plus a revoked one that has
    // not expired: only expiry decides.
    let mut tx = pool.begin().await.expect("a transaction begins");
    refresh_tokens
        .insert(&mut tx, ada.id, LIVE_REFRESH, now + hour)
        .await
        .expect("the live refresh token inserts");
    refresh_tokens
        .insert(&mut tx, ada.id, EXPIRED_REFRESH, now - second)
        .await
        .expect("the expired refresh token inserts");
    let revoked = refresh_tokens
        .insert(&mut tx, ada.id, REVOKED_REFRESH, now + hour)
        .await
        .expect("the revoked refresh token inserts");
    assert!(
        refresh_tokens
            .revoke(&mut tx, revoked.id)
            .await
            .expect("the revocation runs")
    );
    reset_tokens
        .insert(&mut tx, ada.id, LIVE_RESET, now + hour)
        .await
        .expect("the live reset token inserts");
    reset_tokens
        .insert(&mut tx, ada.id, EXPIRED_RESET, now - second)
        .await
        .expect("the expired reset token inserts");
    tx.commit().await.expect("the transaction commits");

    // Three invites: live, expired and unaccepted, and accepted long after
    // its own expiry passed — history rather than garbage.
    let mut tx = pool.begin().await.expect("a transaction begins");
    let live_invite = invites
        .insert(
            &mut tx,
            &Email::parse("kay@example.com").expect("the test email is valid"),
            LIVE_INVITE,
            false,
            None,
            now + hour,
        )
        .await
        .expect("the live invite inserts");
    invites
        .insert(
            &mut tx,
            &Email::parse(EXPIRED_INVITE_EMAIL).expect("the test email is valid"),
            EXPIRED_INVITE,
            false,
            None,
            now - second,
        )
        .await
        .expect("the expired invite inserts");
    let accepted_invite = invites
        .insert(
            &mut tx,
            &Email::parse("linus@example.com").expect("the test email is valid"),
            ACCEPTED_INVITE,
            false,
            None,
            now - hour,
        )
        .await
        .expect("the accepted invite inserts");
    assert!(
        invites
            .mark_accepted(&mut tx, accepted_invite.id, ada.id)
            .await
            .expect("the acceptance runs")
    );
    tx.commit().await.expect("the transaction commits");

    // A user-scoped and a project-scoped secret whose scope rows are about to
    // go, and a global secret, which has no scope row to lose.
    let doomed_user = app
        .insert_user("doomed", "doomed@example.com", false, false)
        .await;
    let mut tx = pool.begin().await.expect("a transaction begins");
    let doomed_project = ProjectRepository::new(pool)
        .insert(
            &mut tx,
            &NewProject::new("doomed", TEST_REMOTE).expect("the test project is valid"),
        )
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    let user_secret = seed_secret(
        pool,
        &fake_secret(ScopeRef::user(doomed_user.id), "USER_TOKEN", "user"),
    )
    .await;
    let project_secret = seed_secret(
        pool,
        &fake_secret(
            ScopeRef::project(doomed_project.id),
            "PROJECT_TOKEN",
            "project",
        ),
    )
    .await;
    let global_secret = seed_secret(
        pool,
        &fake_secret(ScopeRef::global(), "GLOBAL_TOKEN", "global"),
    )
    .await;
    assert_eq!(count_secret_uses(pool).await, 3);

    UserRepository::new(pool)
        .delete(doomed_user.id, ada.id)
        .await
        .expect("the user deletes");
    // Through the repository rather than the project service, which would
    // delete the project's secrets itself; what is left is the residue of a
    // deletion interrupted between the two.
    let mut tx = pool.begin().await.expect("a transaction begins");
    assert!(
        ProjectRepository::new(pool)
            .delete(&mut tx, doomed_project.id)
            .await
            .expect("the project deletes")
    );
    tx.commit().await.expect("the transaction commits");

    let report = app
        .cron()
        .token_cleanup(now)
        .await
        .expect("the sweep succeeds");
    assert_eq!(
        report,
        JobReport {
            items: 5,
            skipped: 0,
            failures: 0,
        },
    );

    // The expired rows went and only they.
    assert!(
        refresh_tokens
            .find_by_hash(EXPIRED_REFRESH)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        refresh_tokens
            .find_by_hash(LIVE_REFRESH)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        refresh_tokens
            .find_by_hash(REVOKED_REFRESH)
            .await
            .unwrap()
            .is_some_and(|token| token.revoked_at.is_some()),
    );
    assert!(
        reset_tokens
            .find_by_hash(EXPIRED_RESET)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        reset_tokens
            .find_by_hash(LIVE_RESET)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(invites.list_open().await.unwrap(), [live_invite]);
    assert!(invite_exists(pool, accepted_invite.id).await);

    assert!(secrets.find(user_secret).await.unwrap().is_none());
    assert!(secrets.find(project_secret).await.unwrap().is_none());
    assert!(secrets.find(global_secret).await.unwrap().is_some());
    // Only the global secret's use survives: the other two cascaded.
    assert_eq!(count_secret_uses(pool).await, 1);

    // The address the expired invite held is free again.
    let mut tx = pool.begin().await.expect("a transaction begins");
    invites
        .insert(
            &mut tx,
            &Email::parse(EXPIRED_INVITE_EMAIL).expect("the test email is valid"),
            REPLACEMENT_INVITE,
            false,
            None,
            now + hour,
        )
        .await
        .expect("the address is free again");
    tx.commit().await.expect("the transaction commits");

    // Nothing left to reap on the next tick.
    let again = app
        .cron()
        .token_cleanup(now)
        .await
        .expect("the second sweep succeeds");
    assert_eq!(again, JobReport::default());
}
