//! `secrets::resolve_for_launch` against a real Postgres: what a profile's
//! `secrets` list and a backend's credential names become when a session
//! starts (`ARCHITECTURE.md`, "Secrets", Resolution at launch and Agent
//! credentials; "Launch sequence"; ADR 0036).
//!
//! The signature says almost nothing about the behaviour that matters here,
//! all of which is about a *group* of rows rather than one row:
//!
//! - precedence — `global`, then `project`, then `user`, last one found wins —
//!   asserted by decrypting what came back and comparing it to the value
//!   stored at the winning scope, so an off-by-one in the ranking cannot pass;
//! - suppression — an `orchestrator_only` winner removes the name entirely and
//!   a lower-precedence injectable row is *not* a fallback, which is the one
//!   way a withheld secret could leak into a container;
//! - a name with no row anywhere warns in exactly the documented words and
//!   lets the launch continue;
//! - the credential names are one slot rather than one each: the most specific
//!   scope wins whichever of them it carries, exactly one row is injected, and
//!   nothing here knows which backend the names belong to, so a launch with no
//!   credential comes back as `credential: None` and the message naming the
//!   backend is the launcher's;
//! - one `secret_uses` row per injected secret per launch, `purpose = launch`,
//!   carrying the session and the creating user — twice when a parked session
//!   is relaunched (`docs/data-model.md`, `secret_uses`).
//!
//! Every value is sealed by the real keyring under the harness's fixed test
//! key and every one of them is obviously fake (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use mars_orchestrator::agent::{backend_for, credential_secret_names};
use mars_orchestrator::models::{
    AgentBackend, NewAgentProfile, NewProject, NewSecret, NewSession, ProfileKind, ScopeRef,
    Secret, SecretName, SecretScope, SecretUsePurpose,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SecretRepository, SessionRepository};
use mars_orchestrator::secrets::{
    LaunchScope, ResolvedSecrets, SealedSecret, SecretIdentity, resolve_for_launch,
};
use uuid::Uuid;

/// Not a real remote: the fixture the project tests use (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// Not a real image: the stub the session fixtures use.
const TEST_IMAGE: &str = "localhost/mars-session:test";

/// The rows one launch needs to exist at all.
struct Fixture {
    user_id: Uuid,
    project_id: Uuid,
    session_id: Uuid,
}

impl Fixture {
    /// The launch scope of this session, with its creating user.
    fn scope(&self) -> LaunchScope {
        LaunchScope {
            session_id: self.session_id,
            project_id: self.project_id,
            created_by: Some(self.user_id),
        }
    }
}

/// A committed user, project, profile and `creating` session, all through
/// their repositories.
async fn seed(app: &TestApp) -> Fixture {
    let suffix = &Uuid::new_v4().simple().to_string()[..8];

    let user = app
        .insert_user(
            &format!("user-{suffix}"),
            &format!("user-{suffix}@example.invalid"),
            false,
            false,
        )
        .await;

    let projects = ProjectRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let project = projects
        .insert(
            &mut tx,
            &NewProject::new(&format!("project-{suffix}"), TEST_REMOTE)
                .expect("the test project is valid"),
        )
        .await
        .expect("the project inserts");
    let profile = projects
        .insert_profile(
            &mut tx,
            &NewAgentProfile::new(project.id, "default", TEST_IMAGE)
                .expect("the test profile is valid"),
        )
        .await
        .expect("the profile inserts");

    let mut new_session = NewSession::new(
        project.id,
        profile.id,
        ProfileKind::Conversational,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );
    new_session.created_by = Some(user.id);
    let session = SessionRepository::new(&app.pool)
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    Fixture {
        user_id: user.id,
        project_id: project.id,
        session_id: session.id,
    }
}

/// Seal `value` for `scope` and commit the row.
async fn seed_secret(
    app: &TestApp,
    scope: ScopeRef,
    raw_name: &str,
    value: &str,
    orchestrator_only: bool,
) -> Secret {
    let secret_name = SecretName::parse(raw_name).expect("the test secret name is valid");
    let sealed = SealedSecret::seal(
        &app.state.keyring,
        SecretIdentity::new(&scope, &secret_name),
        value.as_bytes(),
    )
    .expect("the value seals");

    let mut new = NewSecret::new(sealed);
    new.orchestrator_only = orchestrator_only;

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SecretRepository::new(&app.pool)
        .insert(&mut tx, &new)
        .await
        .expect("the secret inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// The injected value of `name`, or `None` if it was not injected.
fn injected<'a>(resolved: &'a ResolvedSecrets, name: &str) -> Option<&'a str> {
    resolved
        .env
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// The names in `env`, in order.
fn names(resolved: &ResolvedSecrets) -> Vec<&str> {
    resolved.env.iter().map(|(key, _)| key.as_str()).collect()
}

/// Every recorded use of `secret`, newest first.
async fn uses(app: &TestApp, secret: &Secret) -> Vec<mars_orchestrator::models::SecretUse> {
    SecretRepository::new(&app.pool)
        .list_uses(secret.id, 50)
        .await
        .expect("the uses are read")
}

/// A profile's name, as the profile model would yield it.
///
/// The resolver takes `SecretName`s and validates nothing of its own, so a
/// test asks for a name the same way a launch does (`AgentProfile::secret_names`).
fn name(raw: &str) -> SecretName {
    SecretName::parse(raw).expect("the test name is a valid secret name")
}

fn one(raw: &str) -> Vec<SecretName> {
    vec![name(raw)]
}

/// The credential names a Claude session's launch hands the resolver, in the
/// adapter's own order (`agent::credential_secret_names`).
fn claude_credentials() -> Vec<SecretName> {
    credential_secret_names(backend_for(AgentBackend::Claude).as_ref())
}

#[tokio::test]
async fn the_user_scope_wins_over_the_project_scope_and_both_over_global() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;
    let project_secret = seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "DEPLOY_TOKEN",
        "fake-project-value",
        false,
    )
    .await;
    let user_secret = seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "DEPLOY_TOKEN",
        "fake-user-value",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &one("DEPLOY_TOKEN"),
        &[],
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(names(&resolved), vec!["DEPLOY_TOKEN"]);
    assert_eq!(
        injected(&resolved, "DEPLOY_TOKEN"),
        Some("fake-user-value"),
        "the user scope is the last one consulted and therefore wins"
    );
    assert!(resolved.warnings.is_empty());
    assert!(resolved.skipped.is_empty());

    // Only the winner is recorded as read.
    assert_eq!(uses(&app, &user_secret).await.len(), 1);
    assert!(uses(&app, &project_secret).await.is_empty());
}

#[tokio::test]
async fn the_project_scope_wins_when_the_user_has_no_row() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "DEPLOY_TOKEN",
        "fake-project-value",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &one("DEPLOY_TOKEN"),
        &[],
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(
        injected(&resolved, "DEPLOY_TOKEN"),
        Some("fake-project-value")
    );
}

#[tokio::test]
async fn another_projects_secret_is_never_a_candidate() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let other = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::project(other.project_id),
        "DEPLOY_TOKEN",
        "fake-other-project-value",
        false,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::user(other.user_id),
        "DEPLOY_TOKEN",
        "fake-other-user-value",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &one("DEPLOY_TOKEN"),
        &[],
    )
    .await
    .expect("resolution succeeds");

    assert!(resolved.env.is_empty(), "{resolved:?}");
    assert_eq!(
        resolved.warnings,
        vec!["secret DEPLOY_TOKEN is not defined at any scope"]
    );
}

#[tokio::test]
async fn a_user_row_overrides_an_orchestrator_only_global_row_and_is_injected() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-withheld-global-value",
        true,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "DEPLOY_TOKEN",
        "fake-user-value",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &one("DEPLOY_TOKEN"),
        &[],
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(
        injected(&resolved, "DEPLOY_TOKEN"),
        Some("fake-user-value"),
        "precedence decides the winner and only then is its flag read"
    );
    assert!(resolved.skipped.is_empty());
    assert!(resolved.warnings.is_empty());
}

#[tokio::test]
async fn an_orchestrator_only_project_row_suppresses_the_global_value() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let global_secret = seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;
    let project_secret = seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "DEPLOY_TOKEN",
        "fake-withheld-project-value",
        true,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &one("DEPLOY_TOKEN"),
        &[],
    )
    .await
    .expect("resolution succeeds");

    assert!(
        resolved.env.is_empty(),
        "the global value must not leak through a suppressed winner: {resolved:?}"
    );
    assert_eq!(resolved.skipped, vec!["DEPLOY_TOKEN"]);
    assert!(
        resolved.warnings.is_empty(),
        "a withheld secret is deliberate, not a missing one"
    );

    // Nothing was read, so nothing is audited at either scope.
    assert!(uses(&app, &global_secret).await.is_empty());
    assert!(uses(&app, &project_secret).await.is_empty());
}

#[tokio::test]
async fn a_name_with_no_row_anywhere_warns_in_the_documented_words() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let present = seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[name("DEPLOY_TOKEN"), name("ABSENT_TOKEN")],
        &[],
    )
    .await
    .expect("a missing secret does not fail the launch");

    assert_eq!(names(&resolved), vec!["DEPLOY_TOKEN"]);
    assert_eq!(
        resolved.warnings,
        vec!["secret ABSENT_TOKEN is not defined at any scope"],
        "the launcher appends this verbatim as a launch_warning"
    );
    assert!(resolved.skipped.is_empty());

    assert_eq!(
        uses(&app, &present).await.len(),
        1,
        "resolution continues past a missing name"
    );
}

#[tokio::test]
async fn every_injected_secret_is_audited_once_per_launch() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let first = seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;
    let second = seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "ANTHROPIC_API_KEY",
        "fake-api-key-value",
        false,
    )
    .await;
    let withheld = seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "WITHHELD_TOKEN",
        "fake-withheld-value",
        true,
    )
    .await;

    let names_asked = vec![
        name("DEPLOY_TOKEN"),
        name("ANTHROPIC_API_KEY"),
        name("WITHHELD_TOKEN"),
        name("ABSENT_TOKEN"),
    ];

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &names_asked,
        &[],
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(
        names(&resolved),
        vec!["DEPLOY_TOKEN", "ANTHROPIC_API_KEY"],
        "env keeps the profile's order"
    );
    assert_eq!(resolved.skipped, vec!["WITHHELD_TOKEN"]);
    assert_eq!(resolved.warnings.len(), 1);

    for secret in [&first, &second] {
        let recorded = uses(&app, secret).await;
        assert_eq!(recorded.len(), 1, "one row per injected secret");
        assert_eq!(recorded[0].session_id, Some(fixture.session_id));
        assert_eq!(recorded[0].user_id, Some(fixture.user_id));
        assert_eq!(recorded[0].purpose, SecretUsePurpose::Launch);
    }
    assert!(
        uses(&app, &withheld).await.is_empty(),
        "a suppressed secret was never read"
    );

    // A relaunch of a parked session records the reads again
    // (`docs/data-model.md`, `secret_uses`).
    resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &names_asked,
        &[],
    )
    .await
    .expect("the relaunch resolves");

    assert_eq!(uses(&app, &first).await.len(), 2);
    assert_eq!(uses(&app, &second).await.len(), 2);
}

#[tokio::test]
async fn a_session_whose_creator_is_gone_sees_only_global_and_project() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;
    let orphaned = seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "DEPLOY_TOKEN",
        "fake-user-value",
        false,
    )
    .await;

    // `sessions.created_by` is ON DELETE SET NULL, so a deleted user leaves a
    // session with no user scope at all; `secrets.scope_id` is not a foreign
    // key, so the user's own row survives as an orphan.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(fixture.user_id)
        .execute(&app.pool)
        .await
        .expect("the user is deleted");

    let created_by: Option<Uuid> =
        sqlx::query_scalar("SELECT created_by FROM sessions WHERE id = $1")
            .bind(fixture.session_id)
            .fetch_one(&app.pool)
            .await
            .expect("the session is read");
    assert_eq!(created_by, None);

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        LaunchScope {
            session_id: fixture.session_id,
            project_id: fixture.project_id,
            created_by,
        },
        &one("DEPLOY_TOKEN"),
        &[],
    )
    .await
    .expect("resolution succeeds without a user");

    assert_eq!(
        injected(&resolved, "DEPLOY_TOKEN"),
        Some("fake-global-value"),
        "the orphaned user row takes no part in resolution"
    );
    assert!(uses(&app, &orphaned).await.is_empty());

    // The audit row carries the session and no user.
    let recorded = SecretRepository::new(&app.pool)
        .list_uses(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM secrets WHERE scope = 'global' AND name = 'DEPLOY_TOKEN'",
            )
            .fetch_one(&app.pool)
            .await
            .expect("the global row is read"),
            50,
        )
        .await
        .expect("the uses are read");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].session_id, Some(fixture.session_id));
    assert_eq!(recorded[0].user_id, None);
}

#[tokio::test]
async fn a_name_listed_twice_resolves_once() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let secret = seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[
            name("DEPLOY_TOKEN"),
            name("DEPLOY_TOKEN"),
            name("ABSENT_TOKEN"),
            name("ABSENT_TOKEN"),
        ],
        &[],
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(
        names(&resolved),
        vec!["DEPLOY_TOKEN"],
        "env keys are unique"
    );
    assert_eq!(
        resolved.warnings,
        vec!["secret ABSENT_TOKEN is not defined at any scope"],
        "a repeated missing name warns once"
    );
    assert_eq!(
        uses(&app, &secret).await.len(),
        1,
        "and is audited once per launch"
    );
}

#[tokio::test]
async fn an_empty_list_resolves_to_nothing_and_writes_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let secret = seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;

    let resolved = resolve_for_launch(&app.pool, &app.state.keyring, fixture.scope(), &[], &[])
        .await
        .expect("an empty list is not an error");

    assert!(resolved.env.is_empty());
    assert!(resolved.warnings.is_empty());
    assert!(resolved.skipped.is_empty());
    assert!(uses(&app, &secret).await.is_empty());

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM secret_uses")
        .fetch_one(&app.pool)
        .await
        .expect("the audit table is counted");
    assert_eq!(rows, 0, "a profile with no secrets writes no audit row");
}

#[tokio::test]
async fn a_row_the_keyring_cannot_open_fails_the_launch_without_an_audit_trail() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let good = seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;
    let corrupt = seed_secret(
        &app,
        ScopeRef::global(),
        "BROKEN_TOKEN",
        "fake-broken-value",
        false,
    )
    .await;

    // A ciphertext that no longer verifies: the row a rotation left behind, or
    // a corrupted column. Flipping one bit is enough.
    let mut ciphertext = corrupt.ciphertext.clone();
    ciphertext[0] ^= 0x01;
    sqlx::query("UPDATE secrets SET ciphertext = $1 WHERE id = $2")
        .bind(&ciphertext)
        .bind(corrupt.id)
        .execute(&app.pool)
        .await
        .expect("the ciphertext is corrupted");

    let error = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[name("DEPLOY_TOKEN"), name("BROKEN_TOKEN")],
        &[],
    )
    .await
    .expect_err("a value that will not open fails the launch");

    match &error {
        Error::Internal(detail) => assert!(
            !detail.contains("fake-"),
            "the message carries no value: {detail}"
        ),
        other => panic!("expected an internal error, got {other:?}"),
    }

    // The good secret was opened and recorded first; the rollback takes that
    // row with it, so the audit trail never claims a launch that did not
    // happen.
    assert!(
        uses(&app, &good).await.is_empty(),
        "the transaction is rolled back whole"
    );
    assert!(uses(&app, &corrupt).await.is_empty());
}

#[tokio::test]
async fn the_credential_slot_takes_the_most_specific_scope_whatever_its_name() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let global_key = seed_secret(
        &app,
        ScopeRef::global(),
        "ANTHROPIC_API_KEY",
        "not-a-real-api-key",
        false,
    )
    .await;
    let user_token = seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "CLAUDE_CODE_OAUTH_TOKEN",
        "not-a-real-oauth-token",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[],
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(
        names(&resolved),
        vec!["CLAUDE_CODE_OAUTH_TOKEN"],
        "the two names are one slot, so the global API key is not a second entry",
    );
    assert_eq!(
        injected(&resolved, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some("not-a-real-oauth-token"),
    );
    let credential = resolved
        .credential
        .as_ref()
        .expect("the credential is reported");
    assert_eq!(credential.name.as_str(), "CLAUDE_CODE_OAUTH_TOKEN");
    assert_eq!(credential.scope, SecretScope::User);
    assert_eq!(
        resolved.scope_of("CLAUDE_CODE_OAUTH_TOKEN"),
        Some(SecretScope::User),
    );
    assert!(resolved.warnings.is_empty(), "{resolved:?}");
    assert!(resolved.skipped.is_empty());

    // The credential is audited like any other injected secret, and the row
    // that lost the slot was never opened.
    let recorded = uses(&app, &user_token).await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].session_id, Some(fixture.session_id));
    assert_eq!(recorded[0].user_id, Some(fixture.user_id));
    assert_eq!(recorded[0].purpose, SecretUsePurpose::Launch);
    assert!(uses(&app, &global_key).await.is_empty());
}

#[tokio::test]
async fn a_project_credential_beats_a_global_one_and_a_user_one_beats_both() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::global(),
        "ANTHROPIC_API_KEY",
        "not-a-real-global-key",
        false,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "CLAUDE_CODE_OAUTH_TOKEN",
        "not-a-real-project-token",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[],
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(
        injected(&resolved, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some("not-a-real-project-token"),
        "the project row is more specific than the global one",
    );

    // The user's own key now outranks the project's token, name and all.
    seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "ANTHROPIC_API_KEY",
        "not-a-real-user-key",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[],
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(names(&resolved), vec!["ANTHROPIC_API_KEY"]);
    assert_eq!(
        injected(&resolved, "ANTHROPIC_API_KEY"),
        Some("not-a-real-user-key"),
    );
}

#[tokio::test]
async fn the_profile_secrets_and_the_credential_are_injected_together() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::global(),
        "DEPLOY_TOKEN",
        "fake-global-value",
        false,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::global(),
        "CLAUDE_CODE_OAUTH_TOKEN",
        "not-a-real-oauth-token",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &one("DEPLOY_TOKEN"),
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(
        names(&resolved),
        vec!["DEPLOY_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"],
        "the profile keeps its order and the credential follows it",
    );
    assert_eq!(
        resolved
            .credential
            .as_ref()
            .map(|credential| credential.name.as_str()),
        Some("CLAUDE_CODE_OAUTH_TOKEN"),
    );
}

#[tokio::test]
async fn a_name_in_both_lists_is_resolved_once_as_the_credential() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    // A profile that still lists the name is a profile stored before the rule
    // that forbids it (ADR 0036).
    let row = seed_secret(
        &app,
        ScopeRef::global(),
        "ANTHROPIC_API_KEY",
        "not-a-real-api-key",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &one("ANTHROPIC_API_KEY"),
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(names(&resolved), vec!["ANTHROPIC_API_KEY"], "{resolved:?}");
    assert_eq!(
        resolved
            .credential
            .as_ref()
            .map(|credential| credential.name.as_str()),
        Some("ANTHROPIC_API_KEY"),
        "a name in both lists is the credential",
    );
    assert!(resolved.warnings.is_empty());
    assert_eq!(
        uses(&app, &row).await.len(),
        1,
        "one row, one read, one audit row",
    );
}

#[tokio::test]
async fn no_credential_row_anywhere_reports_none_and_warns_about_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[],
        &claude_credentials(),
    )
    .await
    .expect("a session without a credential still launches");

    assert!(resolved.env.is_empty(), "{resolved:?}");
    assert_eq!(resolved.credential, None);
    assert!(
        resolved.warnings.is_empty(),
        "the resolver does not know the backend, so the launcher writes that warning: {resolved:?}",
    );
    assert!(resolved.skipped.is_empty());
}

#[tokio::test]
async fn an_orchestrator_only_credential_is_skipped_without_leaking_the_global_one() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    // Only possible for a row older than the rule that a credential cannot be
    // orchestrator-only (ADR 0036).
    let global_key = seed_secret(
        &app,
        ScopeRef::global(),
        "ANTHROPIC_API_KEY",
        "not-a-real-global-key",
        false,
    )
    .await;
    let withheld = seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "CLAUDE_CODE_OAUTH_TOKEN",
        "not-a-real-withheld-token",
        true,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[],
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds");

    assert!(
        resolved.env.is_empty(),
        "the more specific row won and was withheld; nothing lower takes its place: {resolved:?}",
    );
    assert_eq!(resolved.credential, None);
    assert_eq!(resolved.skipped, vec!["CLAUDE_CODE_OAUTH_TOKEN"]);
    assert!(resolved.warnings.is_empty());
    assert!(uses(&app, &global_key).await.is_empty());
    assert!(uses(&app, &withheld).await.is_empty());
}

#[tokio::test]
async fn two_credentials_at_one_scope_pick_the_backends_first_name() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    // Rows older than the one-credential-per-scope index; the tie is broken by
    // the adapter's order, which puts the subscription token first.
    seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "ANTHROPIC_API_KEY",
        "not-a-real-api-key",
        false,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "CLAUDE_CODE_OAUTH_TOKEN",
        "not-a-real-oauth-token",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        fixture.scope(),
        &[],
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds");

    assert_eq!(names(&resolved), vec!["CLAUDE_CODE_OAUTH_TOKEN"]);
    assert_eq!(
        resolved
            .credential
            .as_ref()
            .map(|credential| credential.name.as_str()),
        Some("CLAUDE_CODE_OAUTH_TOKEN"),
    );
}

#[tokio::test]
async fn a_session_without_a_creator_still_resolves_the_project_credential() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    seed_secret(
        &app,
        ScopeRef::project(fixture.project_id),
        "CLAUDE_CODE_OAUTH_TOKEN",
        "not-a-real-project-token",
        false,
    )
    .await;
    let orphaned = seed_secret(
        &app,
        ScopeRef::user(fixture.user_id),
        "ANTHROPIC_API_KEY",
        "not-a-real-user-key",
        false,
    )
    .await;

    let resolved = resolve_for_launch(
        &app.pool,
        &app.state.keyring,
        LaunchScope {
            session_id: fixture.session_id,
            project_id: fixture.project_id,
            created_by: None,
        },
        &[],
        &claude_credentials(),
    )
    .await
    .expect("resolution succeeds without a user");

    assert_eq!(
        injected(&resolved, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some("not-a-real-project-token"),
        "a deleted creator leaves the project and global scopes",
    );
    assert!(uses(&app, &orphaned).await.is_empty());
}
