//! `/api/secrets` (`SPEC.md`, "Secrets (`/api/secrets`)").
//!
//! A thin adapter over [`SecretsService`], which owns every rule the six
//! endpoints enforce: who may touch which row, what a scope means, what a
//! rename does to the ciphertext and how far back the audit reaches
//! (`src/secrets/service.rs`). What is decided here is only what belongs to
//! HTTP — reading a body or a query string, building the caller's [`Actor`],
//! and choosing the status of a success (201 for a create, 204 for a delete,
//! 200 otherwise, `SPEC.md`, "REST API"). Failures need no mapping at all:
//! the service answers the crate-wide [`Error`], whose `status()` already is
//! the documented code.
//!
//! **No response ever contains `value`** (`SPEC.md`, "Secrets"). That holds
//! structurally rather than by review: every success answers [`SecretMeta`] or
//! the four-field [`SecretUse`] below, neither of which has a value to
//! serialise, and the only types that hold a plaintext are the two request
//! bodies — which carry it as [`Zeroizing<String>`], derive no `Debug`, and
//! hand it to the service, which seals it and drops it (`CLAUDE.md`, rule 3;
//! `ARCHITECTURE.md`, "Secrets", Credential handling). A `?body` in a tracing
//! call on this path does not compile, which is the point.
//!
//! The `admin` half of the [`Actor`] comes from the row [`CurrentUser`]
//! loaded, never from the access token's `admin` claim: a demotion takes
//! effect on the demoted administrator's next request (ADR 0025).

use axum::Router;
use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::routing::{get, put};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::models::{SecretMeta, SecretScope, SecretUse as SecretUseRow, SecretUsePurpose, User};
use crate::prelude::*;
use crate::routes::CurrentUser;
use crate::secrets::service::{Actor, CreateSecret, PatchSecret, SecretsService};

/// The router nested under `/api/secrets`.
///
/// `/{id}` takes the three mutations and `/{id}/uses` the audit read, so the
/// only literal segment is the last one of a two-segment path and no
/// registration order matters here the way it does in `routes::users`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", put(replace).patch(patch).delete(remove))
        .route("/{id}/uses", get(uses))
}

/// The service over this request's pool and keyring.
///
/// Built per handler and dropped with the response: it borrows, holds no state
/// of its own and costs nothing to make.
fn service(state: &AppState) -> SecretsService<'_> {
    SecretsService::new(&state.pool, &state.keyring)
}

/// The caller, as the service's two-field view of them.
fn actor(user: &User) -> Actor {
    Actor::new(user.id, user.admin)
}

// ---- list ----

/// `GET /secrets?scope=&scope_id=`.
///
/// Both parameters are optional and both are the service's to interpret,
/// including the combinations that are refused: `global` with a `scope_id`,
/// `project` without one, and another user's with no `admin` (`SPEC.md`,
/// "Secrets"). A `scope` outside `global|project|user` never reaches a
/// handler — it fails to deserialise and is the [`Query`] rejection's 400.
#[derive(Debug, Deserialize)]
struct ListQuery {
    scope: Option<SecretScope>,
    scope_id: Option<Uuid>,
}

/// `GET /secrets` → the metadata the caller may see.
async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<SecretMeta>>> {
    let listing = service(&state)
        .list(&actor(&user), query.scope, query.scope_id)
        .await?;

    Ok(Json(listing))
}

// ---- create ----

/// `POST /secrets` (`{ scope, scope_id?, name, value, orchestrator_only? }`).
///
/// No `Debug`, deliberately: it holds a plaintext credential, and a derived
/// one would put it in every `?`-formatted tracing field that ever carried
/// this struct (rule 3). `value` is a [`Zeroizing<String>`], so the buffer the
/// body was parsed into is wiped when the request ends, whichever way it ends.
///
/// `deny_unknown_fields` for the reason `routes::users` gives: a client that
/// sends `key_version` or `created_by` here has misunderstood the endpoint —
/// they are the server's — and is told so rather than silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSecretBody {
    scope: SecretScope,
    scope_id: Option<Uuid>,
    name: String,
    #[serde(deserialize_with = "zeroizing_string")]
    value: Zeroizing<String>,
    /// Absent means `false`: a secret is injected into its sessions unless the
    /// caller says otherwise (`docs/data-model.md`, `secrets`).
    #[serde(default)]
    orchestrator_only: bool,
}

/// `POST /secrets` → the stored metadata (201; 409 if the name is taken).
async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<CreateSecretBody>,
) -> Result<(StatusCode, Json<SecretMeta>)> {
    let meta = service(&state)
        .create(
            &actor(&user),
            CreateSecret {
                scope: body.scope,
                scope_id: body.scope_id,
                name: body.name,
                value: body.value,
                orchestrator_only: body.orchestrator_only,
            },
        )
        .await?;

    Ok((StatusCode::CREATED, Json(meta)))
}

// ---- replace ----

/// `PUT /secrets/{id}` (`{ value }`).
///
/// The one field a `PUT` here replaces; the name and the flag are
/// [`PatchBody`]'s. No `Debug`, for the reason [`CreateSecretBody`] gives.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceBody {
    #[serde(deserialize_with = "zeroizing_string")]
    value: Zeroizing<String>,
}

/// `PUT /secrets/{id}` → the stored metadata, with a new value.
async fn replace(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ReplaceBody>,
) -> Result<Json<SecretMeta>> {
    let meta = service(&state)
        .replace_value(&actor(&user), id, body.value)
        .await?;

    Ok(Json(meta))
}

// ---- patch ----

/// `PATCH /secrets/{id}` (`{ name?, orchestrator_only? }`).
///
/// Either field, both or neither; `{}` is legal and answers the current
/// metadata unchanged, which is what a client re-sending a form it did not
/// edit sends. The scope is not among them — changing it is a delete and a
/// create — and neither is `value`, which is [`ReplaceBody`]'s, so
/// `deny_unknown_fields` turns a `PATCH {"value": ...}` into a 400 rather than
/// a silent no-op on a secret the caller believes they just changed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchBody {
    name: Option<String>,
    orchestrator_only: Option<bool>,
}

/// `PATCH /secrets/{id}` → the stored metadata, renamed, re-flagged or both.
async fn patch(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchBody>,
) -> Result<Json<SecretMeta>> {
    let meta = service(&state)
        .patch(
            &actor(&user),
            id,
            PatchSecret {
                name: body.name,
                orchestrator_only: body.orchestrator_only,
            },
        )
        .await?;

    Ok(Json(meta))
}

// ---- delete ----

/// `DELETE /secrets/{id}` → 204.
async fn remove(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    service(&state).delete(&actor(&user), id).await?;

    Ok(StatusCode::NO_CONTENT)
}

// ---- uses ----

/// `GET /secrets/{id}/uses?limit=`.
///
/// Absent means the documented default of 50 and anything above 500 is reduced
/// to it, both in the service; `limit=0` is its 400 and a `limit` that is not
/// a number is the [`Query`] rejection's (`SPEC.md`, "Secrets").
#[derive(Debug, Deserialize)]
struct UsesQuery {
    limit: Option<u32>,
}

/// One audit row as `SPEC.md`, "Secrets" shapes it: `{ session_id, user_id,
/// purpose, at }`.
///
/// A projection rather than the model, which also carries `id` and
/// `secret_id`: the first is a `bigserial` nobody outside the table uses and
/// the second is already in the path, and a response is written from the
/// documented shape rather than from whatever columns the row happens to have.
#[derive(Debug, Serialize)]
struct SecretUse {
    session_id: Option<Uuid>,
    user_id: Option<Uuid>,
    purpose: SecretUsePurpose,
    at: DateTime<Utc>,
}

impl From<SecretUseRow> for SecretUse {
    fn from(row: SecretUseRow) -> Self {
        Self {
            session_id: row.session_id,
            user_id: row.user_id,
            purpose: row.purpose,
            at: row.at,
        }
    }
}

/// `GET /secrets/{id}/uses` → the most recent uses, newest first.
async fn uses(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Query(query): Query<UsesQuery>,
) -> Result<Json<Vec<SecretUse>>> {
    let uses = service(&state).uses(&actor(&user), id, query.limit).await?;

    Ok(Json(uses.into_iter().map(SecretUse::from).collect()))
}

// ---- extraction ----

/// Deserialise a string straight into a [`Zeroizing<String>`].
///
/// `zeroize`'s own `serde` feature is not enabled, and enabling it would only
/// produce this function. The intermediate `String` is *moved* into the
/// wrapper rather than copied, so the allocation the body was parsed into is
/// the one that gets wiped: there is no second live copy of the plaintext.
fn zeroizing_string<'de, D>(deserializer: D) -> std::result::Result<Zeroizing<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Zeroizing::new(String::deserialize(deserializer)?))
}

/// `axum::extract::Query` with [`Error`] as its rejection, so a `scope` that
/// is not one of the three or a `limit` that is not a number answers in the
/// documented `{ status, error }` shape instead of axum's plain text
/// (`SPEC.md`, "REST API").
///
/// The same wrapper `prelude::error` provides for `Json`, for the two
/// extractors this module needs; the `From` conversions it relies on are
/// already there.
struct Query<T>(T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        let axum::extract::Query(value) =
            axum::extract::Query::<T>::from_request_parts(parts, state).await?;

        Ok(Self(value))
    }
}

/// `axum::extract::Path` with [`Error`] as its rejection, so
/// `/secrets/not-a-uuid` is a 400 in the documented shape rather than a
/// plain-text one.
struct Path<T>(T);

impl<T, S> FromRequestParts<S> for Path<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        let axum::extract::Path(value) =
            axum::extract::Path::<T>::from_request_parts(parts, state).await?;

        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The one thing about these DTOs a unit test can prove without a
    /// database: a create body accepts the documented shape and defaults
    /// `orchestrator_only` to `false` when it is left out (`SPEC.md`,
    /// "Secrets"). The value is an obviously fake credential (rule 3).
    #[test]
    fn orchestrator_only_defaults_to_false() {
        let body: CreateSecretBody = serde_json::from_value(json!({
            "scope": "global",
            "name": "DEPLOY_TOKEN",
            "value": "fake-value-not-a-credential",
        }))
        .expect("the documented shape parses");

        assert_eq!(body.scope, SecretScope::Global);
        assert_eq!(body.scope_id, None);
        assert_eq!(body.name, "DEPLOY_TOKEN");
        assert!(!body.orchestrator_only);
        assert_eq!(body.value.as_str(), "fake-value-not-a-credential");
    }

    /// The audit projection carries the four documented fields and neither of
    /// the two the row adds.
    #[test]
    fn a_use_is_projected_to_the_documented_four_fields() {
        let row = SecretUseRow {
            id: 7,
            secret_id: Uuid::from_u128(1),
            session_id: Some(Uuid::from_u128(2)),
            user_id: Some(Uuid::from_u128(3)),
            purpose: SecretUsePurpose::Launch,
            at: Utc::now(),
        };

        let rendered =
            serde_json::to_value(SecretUse::from(row.clone())).expect("the projection serialises");

        assert_eq!(
            rendered,
            json!({
                "session_id": row.session_id,
                "user_id": row.user_id,
                "purpose": "launch",
                "at": row.at,
            })
        );
    }
}
