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

/// The seven fields of `ProfileTemplate` (`SPEC.md`, "Agent profiles"),
/// sorted.
const TEMPLATE_FIELDS: [&str; 7] = [
    "backend",
    "is_default",
    "kind",
    "mcp_tools",
    "name",
    "serves_states",
    "system_prompt",
];

/// The four roles in the order of the table of `SPEC.md`, "Role profile
/// templates", which is the order they are served in.
const ROLES: [&str; 4] = ["planner", "implementer", "reviewer", "merger"];

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
async fn the_four_templates_are_served_in_the_documented_shape_and_order() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let templates = templates(&app, &user).await;

    let names: Vec<&str> = templates
        .iter()
        .map(|t| t["name"].as_str().expect("a template carries a name"))
        .collect();
    assert_eq!(names, ROLES);

    for template in &templates {
        // Both come from `NewAgentProfile::new`, not from the template
        // (`SPEC.md`, "Role profile templates": "Every other field is the
        // documented default").
        assert_eq!(template["kind"], json!("conversational"));
        assert_eq!(template["backend"], json!("claude"));

        let prompt = template["system_prompt"]
            .as_str()
            .expect("a template carries its role prompt");
        assert!(
            !prompt.trim().is_empty(),
            "`{}` has no prompt",
            template["name"]
        );

        assert!(
            !template["serves_states"]
                .as_array()
                .expect("serves_states is an array")
                .is_empty(),
            "`{}` serves no state",
            template["name"],
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
    assert_eq!(templates[0]["serves_states"], json!(["backlog"]));
    assert_eq!(templates[0]["mcp_tools"], json!([]));
    assert_eq!(templates[3]["serves_states"], json!(["merge"]));
    assert_eq!(
        templates[3]["mcp_tools"],
        json!(["list_session_branches", "merge"])
    );

    // Informational, and exactly one of them (`SPEC.md`, "Agent profiles").
    let defaults: Vec<&str> = templates
        .iter()
        .filter(|t| t["is_default"] == json!(true))
        .map(|t| t["name"].as_str().expect("a template carries a name"))
        .collect();
    assert_eq!(defaults, ["implementer"]);
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

    let template = templates(&app, &user).await[0].clone();

    // What the frontend sends on: the template without `is_default`, under a
    // name that is free in this project — the four seeded ones already hold
    // the template names.
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
