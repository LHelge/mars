//! `/api/profile-templates` (`SPEC.md`, "Agent profiles"; "Role profile
//! templates").
//!
//! The one read-only endpoint over [`profile_templates`], so the profile
//! editor can pre-fill a new profile from a role: the way the four roles reach
//! a project that predates the seeding of ADR 0038, and the way a deleted one
//! comes back. Creating the profile is the ordinary
//! `POST /projects/{pid}/profiles`; there is no endpoint that instantiates a
//! template, and the frontend is what checks that a template's served states
//! exist in the target project — an unknown one is already the documented 400
//! of that endpoint.
//!
//! A top-level path rather than `/projects/{pid}/…`: the templates are the
//! same four whatever project is open, and nothing here reads the database.
//!
//! The templates themselves carry only what makes a role a role, so `kind` and
//! `backend` are taken from the profile
//! [`ProfileTemplate::to_new_profile`] builds rather than repeated here: the
//! endpoint reports the defaults a seeded profile really gets, and a changed
//! default reaches this response without being written down twice.

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;
use uuid::Uuid;

use crate::models::{AgentBackend, ProfileKind};
use crate::prelude::*;
use crate::projects::profile_templates;
use crate::routes::CurrentUser;

/// The router nested under `/api/profile-templates`.
pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(list))
}

/// The `ProfileTemplate` of `SPEC.md`, "Agent profiles".
///
/// Deliberately not the whole of `ProfileInput`: a template says what makes
/// the role a role, and every other field of a profile created from one is the
/// documented default. `is_default` is informational — it says which template
/// seeding makes the project's default — and the frontend does not send it on.
#[derive(Debug, Serialize)]
struct ProfileTemplateDto {
    name: &'static str,
    kind: ProfileKind,
    backend: AgentBackend,
    serves_states: &'static [&'static str],
    mcp_tools: &'static [&'static str],
    system_prompt: &'static str,
    is_default: bool,
}

/// `GET /profile-templates` → the four role templates, in template order.
///
/// No database, no project: the answer is the same for every caller who is
/// signed in. Each entry is built through
/// [`ProfileTemplate::to_new_profile`](crate::projects::ProfileTemplate::to_new_profile)
/// over the configured default image, which is where `kind` and `backend` come
/// from and which is also the validation a seeded profile passes, so a
/// template this endpoint would offer is one `POST /projects/{pid}/profiles`
/// accepts.
async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> Result<Json<Vec<ProfileTemplateDto>>> {
    let image = &state.config.session_image_default;

    let templates = profile_templates()
        .iter()
        .map(|template| {
            let defaults = template.to_new_profile(Uuid::nil(), image)?;

            Ok(ProfileTemplateDto {
                name: template.name,
                kind: defaults.kind,
                backend: defaults.backend,
                serves_states: template.serves_states,
                mcp_tools: template.mcp_tools,
                system_prompt: template.system_prompt,
                is_default: template.is_default,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Json(templates))
}
