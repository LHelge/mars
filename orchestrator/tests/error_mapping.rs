//! The wire shape of an error response (`SPEC.md`, "REST API").

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use axum_test::TestServer;
use mars_orchestrator::prelude::*;
use serde_json::json;

async fn conflicting() -> Result<()> {
    Err(Error::Conflict("duplicate name".into()))
}

async fn merge_conflict() -> Result<()> {
    Err(Error::GitConflict {
        message: "merge failed with conflicts".into(),
        conflicts: vec!["src/main.rs".into()],
    })
}

fn server() -> TestServer {
    let app = Router::new()
        .route("/conflict", get(conflicting))
        .route("/git-conflict", get(merge_conflict));
    TestServer::new(app)
}

#[tokio::test]
async fn conflict_answers_409_with_the_documented_body() {
    let response = server().get("/conflict").await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "duplicate name" }));
}

#[tokio::test]
async fn git_conflict_answers_422_with_conflicting_paths() {
    let response = server().get("/git-conflict").await;

    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    response.assert_json(&json!({
        "status": 422,
        "error": "merge failed with conflicts",
        "conflicts": ["src/main.rs"],
    }));
}
