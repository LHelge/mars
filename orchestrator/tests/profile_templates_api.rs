//! `/api/profile-templates` through the real router (`SPEC.md`, "Agent
//! profiles"; "Role profile templates").
//!
//! The read-only endpoint the profile editor pre-fills a new profile from.
//! What the templates *say* is `tests/profile_templates.rs`, which compares
//! them with the document; what is asserted here is the adapter — the path,
//! the JWT requirement and the password-change gate, the documented shape and
//! order, and that a body built from a template is one
//! `POST /projects/{pid}/profiles` accepts, which is the whole point of
//! serving them.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::{AuthenticatedUser, TestApp};
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// The one path.
const PATH: &str = "/api/profile-templates";

/// The ten fields of `ProfileTemplate` (`SPEC.md`, "Agent profiles"),
/// sorted.
const TEMPLATE_FIELDS: [&str; 10] = [
    "auto_launch",
    "backend",
    "is_default",
    "kind",
    "mcp_tools",
    "name",
    "schedule_cron",
    "schedule_prompt",
    "serves_states",
    "system_prompt",
];

/// Every role in the order of the table of `SPEC.md`, "Role profile
/// templates", which is the order they are served in: `claude`, the default,
/// then the four queue roles — the first three of them seeded — then the
/// offered-only scanner.
const ROLES: [&str; 6] = [
    "claude",
    "planner",
    "implementer",
    "reviewer",
    "merger",
    "tech-debt-scanner",
];

/// The scheduled template, which project creation does not seed.
const SCANNER: &str = "tech-debt-scanner";

/// The roles project creation seeds: `claude` and every queue role but the
/// merger, whose `merge` state the orchestrator serves (ADR 0045, 0051).
const SEEDED: [&str; 4] = ["claude", "planner", "implementer", "reviewer"];

/// The two seeded roles the dispatcher launches by itself (ADR 0051).
const AUTO_LAUNCHED: [&str; 2] = ["implementer", "reviewer"];

/// The Claude adapter's preferred credential name, which a schedule needs at
/// `global` or `project` scope (`SPEC.md`, "Agent profiles" → "Scheduled
/// profiles").
const OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Not a real credential (rule 3).
const FAKE_CREDENTIAL: &str = "fake-value-not-a-credential";

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// A signed-in ordinary user to make requests as.
async fn signed_in(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_user(name, &format!("{name}@example.test"), &password(name))
        .await
}

/// `GET /api/profile-templates` as `user`, asserted 200.
async fn templates(app: &TestApp, user: &AuthenticatedUser) -> Vec<Value> {
    let response = app.get_as(user, PATH).await;

    response.assert_status_ok();
    response
        .json::<Value>()
        .as_array()
        .expect("the listing is an array")
        .clone()
}

#[tokio::test]
async fn the_templates_are_served_in_the_documented_shape_and_order() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let templates = templates(&app, &user).await;

    let names: Vec<&str> = templates
        .iter()
        .map(|t| t["name"].as_str().expect("a template carries a name"))
        .collect();
    assert_eq!(names, ROLES);

    for template in &templates {
        // Not from the template (`SPEC.md`, "Role profile templates": "Every
        // other field is the documented default").
        assert_eq!(template["backend"], json!("claude"));
        // The auto-launched roles and the scheduled one run one prompt and
        // end, which is what lets them carry `auto_launch` or a schedule at
        // all; the rest talk to a person.
        let name = template["name"]
            .as_str()
            .expect("a template carries a name");
        let scheduled = name == SCANNER;
        let auto_launched = AUTO_LAUNCHED.contains(&name);
        assert_eq!(
            template["kind"],
            json!(if scheduled || auto_launched {
                "ephemeral"
            } else {
                "conversational"
            }),
            "`{name}`: kind",
        );
        assert_eq!(
            template["auto_launch"],
            json!(auto_launched),
            "`{name}`: auto_launch"
        );

        let prompt = template["system_prompt"]
            .as_str()
            .expect("a template carries its role prompt");
        assert!(
            !prompt.trim().is_empty(),
            "`{}` has no prompt",
            template["name"]
        );

        // Every role serves a queue; the default serves none, so it never
        // sees another role's work (ADR 0051).
        assert_eq!(
            template["serves_states"]
                .as_array()
                .expect("serves_states is an array")
                .is_empty(),
            name == "claude",
            "`{name}`: served states",
        );

        let mut keys: Vec<&str> = template
            .as_object()
            .expect("a template is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, TEMPLATE_FIELDS);
    }

    // The table of "Role profile templates", which the endpoint reports and
    // `tests/profile_templates.rs` checks against the document.
    assert_eq!(templates[0]["serves_states"], json!([]));
    assert_eq!(templates[0]["mcp_tools"], json!([]));
    assert_eq!(templates[1]["serves_states"], json!(["backlog"]));
    assert_eq!(templates[1]["mcp_tools"], json!([]));
    assert_eq!(templates[4]["serves_states"], json!(["merge"]));
    assert_eq!(
        templates[4]["mcp_tools"],
        json!(["list_session_branches", "merge"])
    );

    // The schedule pair: null on every role that is not scheduled, both set
    // on the scanner, whose expression fires once a day.
    for template in &templates[..5] {
        assert_eq!(template["schedule_cron"], json!(null));
        assert_eq!(template["schedule_prompt"], json!(null));
    }
    let scanner = &templates[5];
    assert_eq!(scanner["schedule_cron"], json!("0 4 * * *"));
    assert!(
        !scanner["schedule_prompt"]
            .as_str()
            .expect("the scheduled template carries its run prompt")
            .trim()
            .is_empty(),
    );
    // Filing a task needs no git tool; the task tools are served to every
    // session (`SPEC.md`, "MCP tool contracts").
    assert_eq!(scanner["mcp_tools"], json!([]));

    // Informational, and exactly one of them (`SPEC.md`, "Agent profiles").
    let defaults: Vec<&str> = templates
        .iter()
        .filter(|t| t["is_default"] == json!(true))
        .map(|t| t["name"].as_str().expect("a template carries a name"))
        .collect();
    assert_eq!(defaults, ["claude"]);
}

#[tokio::test]
async fn a_template_is_a_body_the_profiles_endpoint_accepts() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "grace").await;

    let response = app
        .post_as(&user, "/api/projects")
        .json(&json!({ "name": "mars", "remote_url": TEST_REMOTE }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let pid: Uuid = response.json::<Value>()["id"]
        .as_str()
        .expect("a project carries an id")
        .parse()
        .expect("the id is a uuid");

    let template = templates(&app, &user).await[1].clone();

    // What the frontend sends on: the template without `is_default`, under a
    // name that is free in this project — the seeded ones already hold their
    // template names.
    let mut body = template.clone();
    let object = body.as_object_mut().expect("a template is an object");
    object.remove("is_default");
    object.insert("name".into(), json!("planner-2"));

    let created = app
        .post_as(&user, &format!("/api/projects/{pid}/profiles"))
        .json(&body)
        .await;

    created.assert_status(StatusCode::CREATED);
    let created = created.json::<Value>();
    assert_eq!(created["serves_states"], template["serves_states"]);
    assert_eq!(created["mcp_tools"], template["mcp_tools"]);
    assert_eq!(created["system_prompt"], template["system_prompt"]);
    assert_eq!(created["kind"], template["kind"]);
    assert_eq!(created["backend"], template["backend"]);
    // Dropped on the way, so creating from a template never moves the flag.
    assert_eq!(created["is_default"], json!(false));
}

#[tokio::test]
async fn the_scheduled_template_is_a_body_the_profiles_endpoint_accepts() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "hedy").await;

    let response = app
        .post_as(&user, "/api/projects")
        .json(&json!({ "name": "mars", "remote_url": TEST_REMOTE }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let pid: Uuid = response.json::<Value>()["id"]
        .as_str()
        .expect("a project carries an id")
        .parse()
        .expect("the id is a uuid");

    // A schedule needs an agent credential the jobs can resolve without a
    // user, so the scanner is offered to everyone and created by a project
    // that has one (`SPEC.md`, "Agent profiles" → "Scheduled profiles").
    let stored = app
        .post_as(&user, "/api/secrets")
        .json(&json!({
            "scope": "project",
            "scope_id": pid.to_string(),
            "name": OAUTH_TOKEN,
            "value": FAKE_CREDENTIAL,
        }))
        .await;
    stored.assert_status(StatusCode::CREATED);

    let scanner = templates(&app, &user)
        .await
        .into_iter()
        .find(|t| t["name"] == json!(SCANNER))
        .expect("the scheduled template is offered");

    let mut body = scanner.clone();
    body.as_object_mut()
        .expect("a template is an object")
        .remove("is_default");

    let created = app
        .post_as(&user, &format!("/api/projects/{pid}/profiles"))
        .json(&body)
        .await;

    created.assert_status(StatusCode::CREATED);
    let created = created.json::<Value>();
    assert_eq!(created["kind"], json!("ephemeral"));
    assert_eq!(created["schedule_cron"], scanner["schedule_cron"]);
    assert_eq!(created["schedule_prompt"], scanner["schedule_prompt"]);
    assert_eq!(created["serves_states"], scanner["serves_states"]);
    assert_eq!(created["system_prompt"], scanner["system_prompt"]);
    // Stored and read back with the next run the server computed, so no
    // client parses cron.
    assert_ne!(created["next_scheduled_at"], json!(null));
    // The schedule is the scanner's only automation: its template carries
    // `auto_launch` off, and the body sent it on.
    assert_eq!(created["auto_launch"], json!(false));
}

/// An auto-launched template is sent on with its `auto_launch`, so creating
/// one is the create endpoint's ordinary credential rule: refused without an
/// agent credential the dispatcher can use, created with one.
#[tokio::test]
async fn an_auto_launched_template_needs_the_credential_the_endpoint_asks_for() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "radia").await;

    let response = app
        .post_as(&user, "/api/projects")
        .json(&json!({ "name": "mars", "remote_url": TEST_REMOTE }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let pid: Uuid = response.json::<Value>()["id"]
        .as_str()
        .expect("a project carries an id")
        .parse()
        .expect("the id is a uuid");

    let implementer = templates(&app, &user)
        .await
        .into_iter()
        .find(|t| t["name"] == json!("implementer"))
        .expect("the implementer is offered");
    assert_eq!(implementer["auto_launch"], json!(true));

    // What the frontend sends on, under a name the seeded one does not hold.
    let mut body = implementer.clone();
    let object = body.as_object_mut().expect("a template is an object");
    object.remove("is_default");
    object.insert("name".into(), json!("implementer-2"));

    let refused = app
        .post_as(&user, &format!("/api/projects/{pid}/profiles"))
        .json(&body)
        .await;
    refused.assert_status(StatusCode::BAD_REQUEST);
    refused.assert_json(&json!({
        "status": 400,
        "error": "auto_launch requires this backend's agent credential at global or project scope",
    }));

    let stored = app
        .post_as(&user, "/api/secrets")
        .json(&json!({
            "scope": "project",
            "scope_id": pid.to_string(),
            "name": OAUTH_TOKEN,
            "value": FAKE_CREDENTIAL,
        }))
        .await;
    stored.assert_status(StatusCode::CREATED);

    let created = app
        .post_as(&user, &format!("/api/projects/{pid}/profiles"))
        .json(&body)
        .await;
    created.assert_status(StatusCode::CREATED);
    let created = created.json::<Value>();
    assert_eq!(created["kind"], json!("ephemeral"));
    assert_eq!(created["auto_launch"], json!(true));
    assert_eq!(created["serves_states"], json!(["ready"]));
}

#[tokio::test]
async fn the_merger_and_the_scheduled_template_are_not_seeded_into_a_new_project() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "katherine").await;

    let response = app
        .post_as(&user, "/api/projects")
        .json(&json!({ "name": "mars", "remote_url": TEST_REMOTE }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let pid = response.json::<Value>()["id"]
        .as_str()
        .expect("a project carries an id")
        .to_string();

    let profiles = app
        .get_as(&user, &format!("/api/projects/{pid}/profiles"))
        .await;
    profiles.assert_status_ok();
    let names: Vec<String> = profiles
        .json::<Value>()
        .as_array()
        .expect("the listing is an array")
        .iter()
        .map(|p| {
            p["name"]
                .as_str()
                .expect("a profile carries a name")
                .to_string()
        })
        .collect();

    // A schedule spends money on a cadence nobody asked for (ADR 0038), and
    // the seeded `merge` state is merged by the orchestrator (ADR 0045), so
    // creation seeds the default and the three roles of the board and
    // nothing else (ADR 0051).
    assert_eq!(names, SEEDED);
}

#[tokio::test]
async fn the_endpoint_requires_a_token() {
    let app = TestApp::spawn().await;

    let response = app.server.get(PATH).await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&json!({ "status": 401, "error": "authentication required" }));
}

#[tokio::test]
async fn the_endpoint_is_refused_while_a_password_change_is_pending() {
    let app = TestApp::spawn().await;
    let gated = app.create_gated_user("gated", "gated@example.test").await;

    let response = app.get_as(&gated, PATH).await;

    response.assert_status(StatusCode::FORBIDDEN);
    response.assert_json(&json!({ "status": 403, "error": "password change required" }));
}
