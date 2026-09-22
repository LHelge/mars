//! `/api/profile-templates` (`SPEC.md`, "Agent profiles"; "Role profile
//! templates").
//!
//! The one read-only endpoint over [`profile_templates`], so the profile
//! editor can pre-fill a new profile from a role: the way the four seeded
//! roles reach a project that predates the seeding of ADR 0038, the way a
//! deleted one comes back, and the only way the roles project creation does
//! *not* seed — the scheduled `tech-debt-scanner` — reach a project at all.
//! Creating the profile is the ordinary `POST /projects/{pid}/profiles`; there
//! is no endpoint that instantiates a template, and the frontend is what
//! checks that a template's served states exist in the target project — an
//! unknown one is already the documented 400 of that endpoint.
//!
//! A top-level path rather than `/projects/{pid}/…`: the templates are the
//! same whatever project is open, and nothing here reads the database. A
//! scheduled template is offered like any other and refused like any other:
//! `POST /projects/{pid}/profiles` is where the agent credential a schedule
//! needs is checked, so a project with none gets that endpoint's documented
//! 400 and not a shorter list here.
//!
//! The templates themselves carry only what makes a role a role, so `backend`
//! is taken from the profile
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
    /// Null on every role a person launches; a 5-field UTC expression on a
    /// scheduled one (`SPEC.md`, "Agent profiles" → "Scheduled profiles").
    schedule_cron: Option<&'static str>,
    /// The message a scheduled run is given; null exactly when
    /// `schedule_cron` is.
    schedule_prompt: Option<&'static str>,
}

/// `GET /profile-templates` → the role templates, in template order.
///
/// No database, no project: the answer is the same for every caller who is
/// signed in. Each entry is built through
/// [`ProfileTemplate::to_new_profile`](crate::projects::ProfileTemplate::to_new_profile)
/// over the configured default image, which is where `backend` comes from and
/// which is also the validation a seeded profile passes, so a
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
                schedule_cron: template.schedule_cron,
                schedule_prompt: template.schedule_prompt,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Json(templates))
}
