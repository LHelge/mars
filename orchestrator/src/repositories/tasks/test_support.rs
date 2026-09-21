//! The one door left open for the integration tests, and why.
//!
//! [`StateFields`] and `TaskRepository::set_task_state_fields` are
//! `pub(crate)` so that the coupling between a state's kind, the lease,
//! `attempts` and `closed_at` is composed in `tracker/` and nowhere else;
//! `TaskRepository::touch_task_session` and
//! `TaskRepository::append_task_events` are `pub(crate)` because
//! `TrackerMutation::commit` is their only caller. The integration tests in
//! `tests/` are a separate crate, so `pub(crate)` puts all three out of their
//! reach — and there is no tracker verb to drive them through yet: the state
//! change, the claim and the release arrive with the tasks that follow this
//! one.
//!
//! Rather than delete the coverage of rules those tests do assert today — that
//! a release clears both lease columns, that a repeated touch keeps
//! `first_touched_at` — this module hands them a way in. It cannot be gated on
//! the `integration-tests` feature, because CI lints `--all-targets` without
//! that feature too and `tests/` must compile either way; what it is instead
//! is one clearly named door, in one file, that nothing in `src/` calls and
//! that `grep` finds at once. Every method here still demands a [`Locked`], so
//! the tests are held to the same rule as the crate — they are only allowed to
//! say it in one more place.
//!
//! It goes away with the task that moves those tests onto the tracker verbs.

use uuid::Uuid;

use crate::models::{NewTaskEvent, Task};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::{Locked, TaskSessionLinkDto};

pub use super::rows::StateFields;

/// The crate-private tracker writes, for the integration tests only.
#[allow(async_fn_in_trait)]
pub trait TaskRepositoryTestExt {
    /// `TaskRepository::set_task_state_fields`, under the caller's token.
    async fn set_task_state_fields(
        &self,
        locked: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
        fields: &StateFields,
    ) -> Result<Task>;

    /// `TaskRepository::touch_task_session`, under the caller's token.
    async fn touch_task_session(
        &self,
        locked: Locked<'_>,
        task_id: Uuid,
        session_id: Uuid,
    ) -> Result<TaskSessionLinkDto>;

    /// `TaskRepository::append_task_events`, under the caller's token.
    ///
    /// The sequence allocator, its scope check and its single notification are
    /// repository behaviour, asserted here rather than through the typed
    /// `TrackerMutation::emit_*` methods, which cannot express the malformed
    /// and out-of-scope batches those assertions need.
    async fn append_task_events(
        &self,
        locked: Locked<'_>,
        project_id: Uuid,
        events: &[NewTaskEvent],
    ) -> Result<Vec<i64>>;
}

impl TaskRepositoryTestExt for TaskRepository<'_> {
    async fn set_task_state_fields(
        &self,
        locked: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
        fields: &StateFields,
    ) -> Result<Task> {
        TaskRepository::set_task_state_fields(self, locked, project_id, id, fields).await
    }

    async fn touch_task_session(
        &self,
        locked: Locked<'_>,
        task_id: Uuid,
        session_id: Uuid,
    ) -> Result<TaskSessionLinkDto> {
        TaskRepository::touch_task_session(self, locked, task_id, session_id).await
    }

    async fn append_task_events(
        &self,
        locked: Locked<'_>,
        project_id: Uuid,
        events: &[NewTaskEvent],
    ) -> Result<Vec<i64>> {
        TaskRepository::append_task_events(self, locked, project_id, events).await
    }
}
