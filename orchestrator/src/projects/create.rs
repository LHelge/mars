//! Creating a project: one transaction that leaves it ready to be cloned.
//!
//! `POST /api/projects` takes `{name, remote_url, default_branch?,
//! credential?}` and answers a `cloning` project (`SPEC.md`, "Projects"), but
//! a project is never only its row. It starts with the default task states,
//! `merge` among them an auto-merge state sending conflicts back to `ready`
//! (ADR 0045), with the four seeded profiles of [`seeded_profile_templates`]
//! — `claude` (the default), `planner`, `implementer`, `reviewer` — on the
//! built-in Claude image, and —
//! when the caller supplied one — with its remote credential stored as the
//! project-scoped, orchestrator-only secret `GIT_CREDENTIAL`
//! (`docs/data-model.md`, `task_states`, `profile_states`, `agent_profiles`,
//! `secrets`; `SPEC.md`, "User-facing features" → "Agent profiles", "Role
//! profile templates" and "Task states").
//!
//! [`create_project`] is all of that in one `BEGIN … COMMIT`. Either a project
//! exists with everything it needs, or nothing of it does: a duplicate name
//! leaves no orphaned states, profiles or credential behind, and a keyring
//! failure takes the project with it.
//!
//! **Nothing here touches git or the filesystem.** The mirror, the project
//! directory and the branch discovery are the clone job's, which the route
//! spawns after this transaction commits, so a rolled-back creation cannot
//! leave a directory on the volume (`ARCHITECTURE.md`, "Storage").
//!
//! The seeding is application code rather than a trigger or a column default
//! on purpose: the tracker owns what a state set means, and a project that was
//! seeded by the database would have its state list created in one place and
//! edited in another.

use chrono::{TimeDelta, Utc};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::models::{BranchName, NewProject, Project};
use crate::prelude::*;
use crate::projects::profile_templates::seeded_profile_templates;
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::secrets::insert_project_git_credential;
use crate::tracker::Locked;

/// The caller-supplied half of `POST /api/projects` (`SPEC.md`, "Projects").
///
/// Strings rather than the validated models: this is what a request body
/// carries, and [`create_project`] is what turns it into a
/// [`NewProject`] — one place where the field names on the wire meet the
/// rules. `credential` is the remote's password or token, and it is the reason
/// this struct has a hand-written [`std::fmt::Debug`]: a derived one would put
/// the value into any log line, span field or `dbg!` that ever touched the
/// request (rule 3).
pub struct NewProjectRequest {
    pub name: String,
    pub remote_url: String,
    pub default_branch: Option<String>,
    pub credential: Option<String>,
}

impl std::fmt::Debug for NewProjectRequest {
    /// Every field but the credential, which is reduced to whether there was
    /// one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewProjectRequest")
            .field("name", &self.name)
            .field("remote_url", &self.remote_url)
            .field("default_branch", &self.default_branch)
            .field("credential", &self.credential.as_ref().map(|_| "<set>"))
            .finish()
    }
}

/// Create a project and everything it is created with, in one transaction.
///
/// The returned [`Project`] is `cloning` — only the clone job moves it on —
/// and carries the `has_credential` the stored rows justify.
///
/// Validation runs first, outside the transaction, so a malformed name,
/// remote, branch or credential costs no database work and answers 400. A name
/// that is well formed but already taken is the database's answer and comes
/// back as [`Error::Conflict`] from the insert (`SPEC.md`, "Projects").
///
/// The order inside the transaction is deliberate. The credential goes in
/// first, because `has_credential` is not a column: every statement that
/// returns a project computes it with an `EXISTS` over `secrets`, so writing
/// the secret before the row is what makes the returned project tell the truth
/// without a second read ([`ProjectRepository::insert`]). `secrets.scope_id`
/// carries no foreign key, which is what lets the two be written in that order
/// (`docs/data-model.md`, `secrets`). Everything still commits or rolls back
/// together, so nothing observes the intermediate state.
///
/// No project row lock is taken, although the profile and served-state
/// helpers ask for one (`docs/data-model.md`, "Tracker mutation
/// transactions"). The row is being inserted here: it is invisible to every
/// other transaction until this one commits, so there is no concurrent writer
/// for a lock to serialise against, and a [`TrackerMutation`] could not open
/// this transaction anyway — the row it locks does not exist yet. This is the
/// one place that mints its own [`Locked`] token, through
/// [`Locked::during_project_creation`], which is named after this caller
/// precisely so that a second one would stand out.
///
/// No `task_events` row is written for the seeded states either. `states_changed`
/// describes an *edit* to a board somebody is watching, and a project being
/// created has no subscribers and no previous board (`SPEC.md`, "TaskEvent").
#[instrument(skip_all, fields(project_id))]
pub async fn create_project(
    state: &AppState,
    input: NewProjectRequest,
    created_by: Uuid,
) -> Result<Project> {
    // The id is generated here, before anything is written, because it is also
    // the project's directory name under `/data/projects/` and the clone job
    // needs it (`ARCHITECTURE.md`, "Storage").
    let mut new_project = NewProject::new(&input.name, &input.remote_url)?;
    new_project.created_by = Some(created_by);
    new_project.default_branch = input
        .default_branch
        .as_deref()
        .map(BranchName::parse)
        .transpose()?;
    tracing::Span::current().record("project_id", tracing::field::display(new_project.id));

    let credential = credential_buffer(input.credential)?;

    // Through the model's own path, so the defaults of a seeded profile are
    // the documented ones — `claude`, `bypass`, 1800 seconds and the kind's
    // `partial_messages` — rather than literals repeated here
    // (`SPEC.md`, "Agent profiles"), and so a template that ever stopped
    // validating fails the creation instead of reaching the column.
    //
    // The timestamps are spaced one microsecond apart in template order, which
    // is the order they are listed in: `now()` is the transaction's start
    // and would give all of them the same value, leaving `ORDER BY created_at,
    // name` to sort them alphabetically (`NewAgentProfile::created_at`).
    let seeded_at = Utc::now();
    let profiles: Vec<_> = seeded_profile_templates()
        .enumerate()
        .map(|(index, template)| {
            let mut profile =
                template.to_new_profile(new_project.id, &state.config.session_image_default)?;
            profile.created_at = Some(seeded_at + TimeDelta::microseconds(index as i64));

            Ok(profile)
        })
        .collect::<Result<_>>()?;

    let projects = ProjectRepository::new(&state.pool);
    let tasks = TaskRepository::new(&state.pool);

    let mut tx = state.pool.begin().await?;

    if let Some(value) = &credential {
        insert_project_git_credential(
            &state.pool,
            &mut tx,
            &state.keyring,
            new_project.id,
            value,
            Some(created_by),
        )
        .await?;
    }
    // Sealed and stored; the plaintext is of no further use to this function
    // and its buffer is wiped here rather than at the end of the scope
    // (`ARCHITECTURE.md`, "Secrets", Credential handling).
    drop(credential);

    let project = projects.insert(&mut tx, &new_project).await?;
    tasks
        .insert_default_states(Locked::during_project_creation(&mut tx), project.id)
        .await?;

    for profile in &profiles {
        let inserted = projects.insert_profile(&mut tx, profile).await?;
        tasks
            .set_profile_states_by_name(
                Locked::during_project_creation(&mut tx),
                project.id,
                inserted.id,
                &profile.serves_states,
            )
            .await?;
    }

    tx.commit().await?;

    info!(
        project_id = %project.id,
        profiles = profiles.len(),
        has_credential = project.has_credential,
        "project created"
    );

    Ok(project)
}

/// The supplied credential in a zeroized buffer, or [`Error::BadRequest`] when
/// the field was present but blank.
///
/// Omitting the field is how a public repository is created, so `None` is not
/// an error; sending an empty or whitespace-only one is a client that meant to
/// send something. Trimming decides only *whether* there is a value: what is
/// stored is exactly what arrived, because leading and trailing whitespace can
/// be significant in a token, and how long a secret value may be is the
/// secrets module's rule, applied when the row is sealed.
///
/// The `String` is moved into the [`Zeroizing`] buffer rather than copied, so
/// the one allocation that ever held the plaintext is the one that is wiped.
/// Neither the value nor its length reaches the message (rule 3).
fn credential_buffer(raw: Option<String>) -> Result<Option<Zeroizing<String>>> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let value = Zeroizing::new(raw);
    if value.trim().is_empty() {
        return Err(Error::BadRequest("credential must not be empty".into()));
    }

    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_credential_is_not_an_error() {
        assert!(
            credential_buffer(None)
                .expect("a public repository needs no credential")
                .is_none()
        );
    }

    #[test]
    fn a_credential_is_kept_exactly_as_it_arrived() {
        // Obviously fake, and deliberately padded: the padding is stored
        // (rule 3).
        let value = credential_buffer(Some(" fake-git-credential-for-tests ".to_string()))
            .expect("a non-blank credential is accepted")
            .expect("there is a value");

        assert_eq!(value.as_str(), " fake-git-credential-for-tests ");
    }

    #[test]
    fn a_blank_credential_is_a_bad_request() {
        for raw in ["", " ", "\t\n "] {
            let error = credential_buffer(Some(raw.to_string()))
                .expect_err("a blank credential is rejected");

            assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
            assert_eq!(error.to_string(), "credential must not be empty");
        }
    }

    #[test]
    fn the_debug_rendering_never_carries_the_credential() {
        let request = NewProjectRequest {
            name: "mars".to_string(),
            remote_url: "https://example.invalid/org/repo.git".to_string(),
            default_branch: None,
            credential: Some("fake-git-credential-for-tests".to_string()),
        };

        let rendered = format!("{request:?}");
        assert!(!rendered.contains("fake-git-credential"), "{rendered}");
        assert!(rendered.contains("mars"), "{rendered}");
    }
}
