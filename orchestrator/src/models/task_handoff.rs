//! Code hand-offs: an immutable record of committed work passed between
//! workers, with the review status of that exact commit.
//!
//! `docs/data-model.md`, `task_handoffs` and `SPEC.md`, "Code hand-offs and
//! review" (ADR 0018). Publishing a revision starts unreviewed; forwarding
//! copies the source and commit and either carries the existing review
//! attribution forward or records a new decision. Which record may be
//! forwarded, and whether the referenced rows share a project, is decided in
//! the repository under the project lock.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::task::{TaskError, TaskResult};
// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// The two full object id lengths git produces: SHA-1 and SHA-256.
const COMMIT_ID_LENGTHS: [usize; 2] = [40, 64];

/// Is `raw` a full lowercase hexadecimal git object id?
///
/// Abbreviated ids and uppercase hex are rejected: a hand-off pins one exact
/// commit (`SPEC.md`, "Code hand-offs and review"), and git itself writes
/// lowercase.
pub fn is_commit_id(raw: &str) -> bool {
    COMMIT_ID_LENGTHS.contains(&raw.len())
        && raw.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// The review status of a hand-off's commit.
///
/// A `TEXT` column with a `CHECK`, not a database enum type
/// (`docs/data-model.md`, `task_handoffs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, Default)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Not reviewed yet; the state a new revision always starts in.
    #[default]
    Unreviewed,
    /// This commit was approved; a task merge requires it.
    Approved,
    /// This commit was rejected; the next implementer starts from it.
    ChangesRequested,
}

impl ReviewStatus {
    /// The stored `TEXT` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreviewed => "unreviewed",
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
        }
    }

    /// Has someone made a decision about this commit?
    pub fn is_reviewed(self) -> bool {
        !matches!(self, Self::Unreviewed)
    }
}

impl std::fmt::Display for ReviewStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `task_handoffs` row, column for column (`docs/data-model.md`).
///
/// Rows are immutable except for foreign keys becoming NULL on deletion, which
/// is why every actor is optional here while insertion requires exactly one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskHandoff {
    pub id: Uuid,
    pub task_id: Uuid,
    pub source_session_id: Option<Uuid>,
    pub source_branch: String,
    pub commit: String,
    pub comment_id: Option<Uuid>,
    pub review_status: ReviewStatus,
    pub reviewed_by_user_id: Option<Uuid>,
    pub reviewed_by_session_id: Option<Uuid>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub created_by_user_id: Option<Uuid>,
    pub created_by_session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// The caller-supplied half of a new hand-off record.
///
/// `comment_id` is not optional here: a hand-off is always published with its
/// comment, and only deletion can null the column later. The id is generated up
/// front because the commit is retained at `refs/handoffs/<id>` before the task
/// moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskHandoff {
    pub id: Uuid,
    pub task_id: Uuid,
    pub source_session_id: Option<Uuid>,
    pub source_branch: String,
    pub commit: String,
    pub comment_id: Uuid,
    pub review_status: ReviewStatus,
    pub reviewed_by_user_id: Option<Uuid>,
    pub reviewed_by_session_id: Option<Uuid>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub created_by_user_id: Option<Uuid>,
    pub created_by_session_id: Option<Uuid>,
}

impl NewTaskHandoff {
    /// A fresh, unreviewed record for `commit` on `source_branch`.
    ///
    /// This is a revision publication: a new revision never carries an old
    /// approval. The caller sets the creating actor and, when forwarding, the
    /// review fields, then calls [`NewTaskHandoff::validate`].
    pub fn new(
        task_id: Uuid,
        source_branch: impl Into<String>,
        commit: impl Into<String>,
        comment_id: Uuid,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            task_id,
            source_session_id: None,
            source_branch: source_branch.into(),
            commit: commit.into(),
            comment_id,
            review_status: ReviewStatus::Unreviewed,
            reviewed_by_user_id: None,
            reviewed_by_session_id: None,
            reviewed_at: None,
            created_by_user_id: None,
            created_by_session_id: None,
        }
    }

    /// Every rule that needs only this record: a pinned commit, one creating
    /// actor, a source branch, and review fields that match the status.
    ///
    /// Same-project membership of the task, comment and sessions, and the
    /// "forwarding matches the current hand-off" rule, need the other rows and
    /// belong to the repository.
    pub fn validate(&self) -> TaskResult<()> {
        if !is_commit_id(&self.commit) {
            return Err(TaskError::InvalidCommit);
        }

        if self.source_branch.trim().is_empty() {
            return Err(TaskError::EmptySourceBranch);
        }

        let actors = usize::from(self.created_by_user_id.is_some())
            + usize::from(self.created_by_session_id.is_some());
        if actors != 1 {
            return Err(TaskError::InvalidHandoffActor);
        }

        let reviewers = usize::from(self.reviewed_by_user_id.is_some())
            + usize::from(self.reviewed_by_session_id.is_some());
        let reviewed = self.review_status.is_reviewed();
        if reviewers != usize::from(reviewed) || self.reviewed_at.is_some() != reviewed {
            return Err(TaskError::InvalidReview);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// An obviously fake but well-formed SHA-1 object id.
    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";
    /// An obviously fake but well-formed SHA-256 object id.
    const SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn published() -> NewTaskHandoff {
        let mut handoff = NewTaskHandoff::new(
            Uuid::new_v4(),
            "session/11111111-1111-4111-8111-111111111111",
            SHA1,
            Uuid::new_v4(),
        );
        handoff.created_by_session_id = Some(Uuid::new_v4());
        handoff
    }

    #[test]
    fn review_status_serialises_as_the_stored_text() {
        assert_eq!(
            serde_json::to_value(ReviewStatus::Unreviewed).unwrap(),
            json!("unreviewed")
        );
        assert_eq!(
            serde_json::to_value(ReviewStatus::Approved).unwrap(),
            json!("approved")
        );
        assert_eq!(
            serde_json::to_value(ReviewStatus::ChangesRequested).unwrap(),
            json!("changes_requested")
        );
        assert_eq!(
            serde_json::from_value::<ReviewStatus>(json!("changes_requested")).unwrap(),
            ReviewStatus::ChangesRequested
        );
        for status in [
            ReviewStatus::Unreviewed,
            ReviewStatus::Approved,
            ReviewStatus::ChangesRequested,
        ] {
            assert_eq!(status.to_string(), status.as_str());
        }
        assert_eq!(ReviewStatus::default(), ReviewStatus::Unreviewed);
        assert!(!ReviewStatus::Unreviewed.is_reviewed());
        assert!(ReviewStatus::Approved.is_reviewed());
        assert!(ReviewStatus::ChangesRequested.is_reviewed());
    }

    #[test]
    fn a_published_revision_is_unreviewed_and_valid() {
        let handoff = published();
        assert_eq!(handoff.review_status, ReviewStatus::Unreviewed);
        assert!(handoff.reviewed_at.is_none());
        assert!(handoff.validate().is_ok());
    }

    #[test]
    fn both_full_object_id_lengths_are_accepted() {
        let mut handoff = published();
        assert!(handoff.validate().is_ok());

        handoff.commit = SHA256.to_string();
        assert!(handoff.validate().is_ok());
    }

    #[test]
    fn abbreviated_uppercase_and_non_hex_commits_are_rejected() {
        let mut handoff = published();
        for commit in [
            "",
            "0123456",
            &SHA1[..39],
            &format!("{SHA1}0"),
            &SHA1.to_uppercase(),
            "0123456789ABCDEF0123456789abcdef01234567",
            "0123456789abcdefg123456789abcdef01234567",
            "0123456789abcdef 123456789abcdef01234567",
        ] {
            handoff.commit = commit.to_string();
            assert_eq!(
                handoff.validate(),
                Err(TaskError::InvalidCommit),
                "accepted {commit:?}"
            );
        }
    }

    #[test]
    fn an_empty_source_branch_is_rejected() {
        let mut handoff = published();
        handoff.source_branch = "   ".to_string();
        assert_eq!(handoff.validate(), Err(TaskError::EmptySourceBranch));

        handoff.source_branch = String::new();
        assert_eq!(handoff.validate(), Err(TaskError::EmptySourceBranch));
    }

    #[test]
    fn exactly_one_creating_actor_is_required() {
        let mut handoff = published();

        handoff.created_by_user_id = Some(Uuid::new_v4());
        assert_eq!(handoff.validate(), Err(TaskError::InvalidHandoffActor));

        handoff.created_by_session_id = None;
        assert!(handoff.validate().is_ok());

        handoff.created_by_user_id = None;
        assert_eq!(handoff.validate(), Err(TaskError::InvalidHandoffActor));
    }

    #[test]
    fn an_unreviewed_record_has_no_reviewer_and_no_review_time() {
        let mut handoff = published();

        handoff.reviewed_by_user_id = Some(Uuid::new_v4());
        assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));

        handoff.reviewed_by_user_id = None;
        handoff.reviewed_at = Some(Utc::now());
        assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));
    }

    #[test]
    fn a_reviewed_record_has_exactly_one_reviewer_and_a_review_time() {
        for status in [ReviewStatus::Approved, ReviewStatus::ChangesRequested] {
            let mut handoff = published();
            handoff.review_status = status;

            // Decision without reviewer or time.
            assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));

            // Reviewer without a time.
            handoff.reviewed_by_user_id = Some(Uuid::new_v4());
            assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));

            // Reviewer and time.
            handoff.reviewed_at = Some(Utc::now());
            assert!(handoff.validate().is_ok());

            // A session reviewer works the same way.
            handoff.reviewed_by_user_id = None;
            handoff.reviewed_by_session_id = Some(Uuid::new_v4());
            assert!(handoff.validate().is_ok());

            // Two reviewers.
            handoff.reviewed_by_user_id = Some(Uuid::new_v4());
            assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));

            // A review time with no reviewer.
            handoff.reviewed_by_user_id = None;
            handoff.reviewed_by_session_id = None;
            assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));
        }
    }
}
