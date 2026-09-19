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
    /// Whether the review fields were copied from the record being forwarded
    /// rather than decided by this caller.
    ///
    /// Not a column: `task_handoffs` stores what the review *is*, not where it
    /// came from. It exists because the one rule it relaxes cannot be decided
    /// from the other fields. "Forwarding preserves attribution even when a
    /// prior reviewer has since been deleted" (`docs/data-model.md`,
    /// `task_handoffs`), and deletion nulls `reviewed_by_user_id` and
    /// `reviewed_by_session_id` — so a carried decision can legitimately be a
    /// reviewed status with a `reviewed_at` and *no* reviewer, which is exactly
    /// what [`NewTaskHandoff::validate`] refuses for a decision a caller makes
    /// now. Set only by [`NewTaskHandoff::carry_review`].
    pub carried: bool,
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
            carried: false,
        }
    }

    /// Carry `previous`'s review verdict, reviewer and time onto this record.
    ///
    /// A forward with no decision of its own: "no decision means the previous
    /// review status and reviewer are carried forward" (`SPEC.md`, "Code
    /// hand-offs and review"). The four fields are copied verbatim, including
    /// a reviewer pair that deletion has already nulled, and [`Self::carried`]
    /// records that they were copied so that [`NewTaskHandoff::validate`] lets
    /// that one shape through.
    pub fn carry_review(&mut self, previous: &TaskHandoff) {
        self.review_status = previous.review_status;
        self.reviewed_by_user_id = previous.reviewed_by_user_id;
        self.reviewed_by_session_id = previous.reviewed_by_session_id;
        self.reviewed_at = previous.reviewed_at;
        self.carried = true;
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
        if self.reviewed_at.is_some() != reviewed {
            return Err(TaskError::InvalidReview);
        }
        // A decision made here names exactly one reviewer. A decision *carried*
        // from the record being forwarded names at most one, because deletion
        // nulls the reviewer and the verdict outlives the reviewer.
        let reviewers_allowed = match (reviewed, self.carried) {
            (true, true) => reviewers <= 1,
            (reviewed, _) => reviewers == usize::from(reviewed),
        };
        if !reviewers_allowed {
            return Err(TaskError::InvalidReview);
        }

        Ok(())
    }
}

/// An explicit review decision recorded while forwarding a hand-off.
///
/// The input half of [`ReviewStatus`]: a caller may record a decision but
/// never "unreviewed", which is only ever the state a fresh revision starts in
/// (`SPEC.md`, "Code hand-offs and review").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// This commit is good; a task merge may use it.
    Approved,
    /// This commit needs more work; the next implementer starts from it.
    ChangesRequested,
}

impl From<ReviewDecision> for ReviewStatus {
    fn from(decision: ReviewDecision) -> Self {
        match decision {
            ReviewDecision::Approved => Self::Approved,
            ReviewDecision::ChangesRequested => Self::ChangesRequested,
        }
    }
}

/// The `HandoffInput` union of `SPEC.md`, "Code hand-offs and review".
///
/// The same JSON shape reaches the tracker two ways: as the `handoff` field of
/// `PUT /projects/{pid}/tasks/{id}` (`SPEC.md`, "Tasks") and as the `handoff`
/// field of the MCP `update` tool's input object (`SPEC.md`, "MCP tool
/// contracts"). Both hand it to [`HandoffInput::validate`] with the
/// [`HandoffCaller`] they authenticated: the route builds
/// [`HandoffCaller::User`] from its `CurrentUser`, the tool handler builds
/// [`HandoffCaller::Session`] from its `SessionContext`. Everything that needs
/// the database — that the source session belongs to the task's project, that
/// a forward names the task's *current* hand-off, that the commit really is
/// the session tip — is decided later, under the locks.
///
/// Unknown fields are accepted, as everywhere else in the API. That is why the
/// `revision` variant carries a `review` field the contract does not give it:
/// without capturing it, a review decision sent with a revision would be
/// silently dropped instead of refused with
/// [`TaskError::HandoffReviewOnRevision`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HandoffInput {
    /// Publish committed work as the task's new hand-off, always unreviewed.
    Revision {
        /// The session the commit comes from. Required over REST, rejected
        /// over MCP, where it is the calling session.
        #[serde(default)]
        source_session_id: Option<Uuid>,
        /// The full git object id to pin, not a moving ref.
        commit: String,
        /// The message to the next agent.
        comment: String,
        /// Never part of a revision; captured only to reject it by name.
        #[serde(default)]
        review: Option<ReviewDecision>,
    },
    /// Pass the task's current hand-off on, optionally reviewing its commit.
    Forward {
        /// Must equal the task's current hand-off; checked under the lock.
        handoff_id: Uuid,
        /// The message to the next agent.
        comment: String,
        /// Absent carries the existing review status and reviewer forward.
        #[serde(default)]
        review: Option<ReviewDecision>,
    },
}

/// Who is publishing a hand-off, and therefore where the source session
/// comes from.
///
/// The ids are taken from authenticated context, never from input
/// (`SPEC.md`, "Code hand-offs and review").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffCaller {
    /// A REST caller: a user, who names the source session explicitly.
    User {
        /// The authenticated user.
        user_id: Uuid,
    },
    /// An MCP caller: a session, which *is* the source session.
    Session {
        /// The calling session.
        session_id: Uuid,
    },
}

/// A [`HandoffInput`] that passed every rule answerable without the database.
///
/// The strings are trimmed and the source session is resolved, so the caller
/// that receives one does not have to remember which half of the contract it
/// still owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedHandoff {
    /// A revision publication at one pinned commit.
    Revision {
        /// Supplied by a user, or the calling session itself.
        source_session_id: Uuid,
        /// A full lowercase hexadecimal git object id.
        commit: String,
        /// Trimmed and non-empty.
        comment: String,
    },
    /// A forward of the hand-off the task currently carries.
    Forward {
        /// The hand-off the caller believes is current.
        handoff_id: Uuid,
        /// Trimmed and non-empty.
        comment: String,
        /// An explicit decision, or none to carry the old one forward.
        review: Option<ReviewDecision>,
    },
}

impl HandoffInput {
    /// Every hand-off rule that needs neither the database nor the git repo.
    ///
    /// A non-empty comment for both variants; for a revision, a pinned commit,
    /// no review decision, and a source session that is present exactly when
    /// the caller is a user.
    pub fn validate(&self, caller: &HandoffCaller) -> TaskResult<ValidatedHandoff> {
        let comment = match self {
            Self::Revision { comment, .. } | Self::Forward { comment, .. } => comment.trim(),
        };
        if comment.is_empty() {
            return Err(TaskError::EmptyComment);
        }
        let comment = comment.to_string();

        match self {
            Self::Revision {
                source_session_id,
                commit,
                review,
                ..
            } => {
                if review.is_some() {
                    return Err(TaskError::HandoffReviewOnRevision);
                }
                if !is_commit_id(commit) {
                    return Err(TaskError::InvalidCommit);
                }

                // An MCP caller never names a source session, not even its
                // own: the id comes from the authenticated context.
                let source_session_id = match (caller, source_session_id) {
                    (HandoffCaller::User { .. }, Some(id)) => *id,
                    (HandoffCaller::User { .. }, None) => {
                        return Err(TaskError::HandoffSourceRequired);
                    }
                    (HandoffCaller::Session { .. }, Some(_)) => {
                        return Err(TaskError::HandoffSourceNotAllowed);
                    }
                    (HandoffCaller::Session { session_id }, None) => *session_id,
                };

                Ok(ValidatedHandoff::Revision {
                    source_session_id,
                    commit: commit.clone(),
                    comment,
                })
            }
            Self::Forward {
                handoff_id, review, ..
            } => Ok(ValidatedHandoff::Forward {
                handoff_id: *handoff_id,
                comment,
                review: *review,
            }),
        }
    }

    /// A hand-off "requires a different target `state` in the same update"
    /// (`SPEC.md`, "Code hand-offs and review").
    ///
    /// One message for both ways of failing it — no `state` at all, and the
    /// state the task is already in — because a hand-off that does not move
    /// the task is the same mistake either way. The comparison is on exact
    /// names: a name that differs only in case or whitespace is a *different*
    /// state name, and fails later as an unknown state.
    ///
    /// The route and the MCP `update` tool call this before any git work, so
    /// that a doomed request never syncs or retains a commit.
    pub fn require_state_change(
        target_state: Option<&str>,
        current_state_name: &str,
    ) -> TaskResult<()> {
        match target_state {
            Some(name) if name != current_state_name => Ok(()),
            _ => Err(TaskError::HandoffRequiresStateChange),
        }
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

    /// A user caller with a fixed, obviously fake id.
    fn user_caller() -> HandoffCaller {
        HandoffCaller::User {
            user_id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        }
    }

    /// A session caller with a fixed, obviously fake id.
    fn session_caller() -> (HandoffCaller, Uuid) {
        let session_id = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
        (HandoffCaller::Session { session_id }, session_id)
    }

    #[test]
    fn review_decision_maps_onto_the_stored_review_status() {
        assert_eq!(
            serde_json::to_value(ReviewDecision::Approved).unwrap(),
            json!("approved")
        );
        assert_eq!(
            serde_json::to_value(ReviewDecision::ChangesRequested).unwrap(),
            json!("changes_requested")
        );
        assert_eq!(
            serde_json::from_value::<ReviewDecision>(json!("approved")).unwrap(),
            ReviewDecision::Approved
        );
        assert_eq!(
            ReviewStatus::from(ReviewDecision::Approved),
            ReviewStatus::Approved
        );
        assert_eq!(
            ReviewStatus::from(ReviewDecision::ChangesRequested),
            ReviewStatus::ChangesRequested
        );
        // "unreviewed" is a status, never a decision a caller may send.
        assert!(serde_json::from_value::<ReviewDecision>(json!("unreviewed")).is_err());
    }

    #[test]
    fn both_input_variants_round_trip_through_serde() {
        let source = Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap();
        let revision = HandoffInput::Revision {
            source_session_id: Some(source),
            commit: SHA1.to_string(),
            comment: "Ready for review.".to_string(),
            review: None,
        };
        assert_eq!(
            serde_json::to_value(&revision).unwrap(),
            json!({
                "kind": "revision",
                "source_session_id": source,
                "commit": SHA1,
                "comment": "Ready for review.",
                "review": null,
            })
        );
        assert_eq!(
            serde_json::from_value::<HandoffInput>(json!({
                "kind": "revision",
                "source_session_id": source,
                "commit": SHA1,
                "comment": "Ready for review.",
            }))
            .unwrap(),
            revision
        );

        let handoff_id = Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap();
        let forward = HandoffInput::Forward {
            handoff_id,
            comment: "Looks good.".to_string(),
            review: Some(ReviewDecision::Approved),
        };
        assert_eq!(
            serde_json::to_value(&forward).unwrap(),
            json!({
                "kind": "forward",
                "handoff_id": handoff_id,
                "comment": "Looks good.",
                "review": "approved",
            })
        );
        assert_eq!(
            serde_json::from_value::<HandoffInput>(json!({
                "kind": "forward",
                "handoff_id": handoff_id,
                "comment": "Looks good.",
                "review": "approved",
            }))
            .unwrap(),
            forward
        );

        // `source_session_id` and `review` are the only optional fields.
        assert!(
            serde_json::from_value::<HandoffInput>(json!({
                "kind": "revision",
                "commit": SHA1,
                "comment": "Ready.",
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<HandoffInput>(json!({
                "kind": "forward",
                "handoff_id": handoff_id,
                "comment": "Looks good.",
            }))
            .is_ok()
        );
    }

    #[test]
    fn an_unknown_kind_or_a_missing_field_does_not_deserialise() {
        for body in [
            json!({ "commit": SHA1, "comment": "Ready." }),
            json!({ "kind": "publish", "commit": SHA1, "comment": "Ready." }),
            json!({ "kind": "revision", "comment": "Ready." }),
            json!({ "kind": "revision", "commit": SHA1 }),
            json!({ "kind": "forward", "comment": "Ready." }),
            json!({ "kind": "forward", "handoff_id": Uuid::nil() }),
        ] {
            assert!(
                serde_json::from_value::<HandoffInput>(body.clone()).is_err(),
                "accepted {body}"
            );
        }
    }

    #[test]
    fn unknown_fields_are_accepted_as_everywhere_else_in_the_api() {
        assert!(
            serde_json::from_value::<HandoffInput>(json!({
                "kind": "forward",
                "handoff_id": Uuid::nil(),
                "comment": "Looks good.",
                "commit": SHA1,
            }))
            .is_ok()
        );
    }

    #[test]
    fn a_user_revision_carries_its_source_session_and_a_trimmed_comment() {
        let source = Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap();
        let input = HandoffInput::Revision {
            source_session_id: Some(source),
            commit: SHA256.to_string(),
            comment: "  Ready for review.\n".to_string(),
            review: None,
        };

        assert_eq!(
            input.validate(&user_caller()),
            Ok(ValidatedHandoff::Revision {
                source_session_id: source,
                commit: SHA256.to_string(),
                comment: "Ready for review.".to_string(),
            })
        );
    }

    #[test]
    fn a_user_revision_without_a_source_session_is_rejected() {
        let input = HandoffInput::Revision {
            source_session_id: None,
            commit: SHA1.to_string(),
            comment: "Ready.".to_string(),
            review: None,
        };

        assert_eq!(
            input.validate(&user_caller()),
            Err(TaskError::HandoffSourceRequired)
        );
        assert_eq!(
            TaskError::HandoffSourceRequired.to_string(),
            "revision hand-off requires source_session_id"
        );
    }

    #[test]
    fn a_session_revision_takes_its_source_from_the_caller() {
        let (caller, session_id) = session_caller();
        let input = HandoffInput::Revision {
            source_session_id: None,
            commit: SHA1.to_string(),
            comment: "Ready.".to_string(),
            review: None,
        };

        assert_eq!(
            input.validate(&caller),
            Ok(ValidatedHandoff::Revision {
                source_session_id: session_id,
                commit: SHA1.to_string(),
                comment: "Ready.".to_string(),
            })
        );
    }

    #[test]
    fn a_session_revision_may_not_name_a_source_session_even_its_own() {
        let (caller, session_id) = session_caller();

        for source in [session_id, Uuid::new_v4()] {
            let input = HandoffInput::Revision {
                source_session_id: Some(source),
                commit: SHA1.to_string(),
                comment: "Ready.".to_string(),
                review: None,
            };
            assert_eq!(
                input.validate(&caller),
                Err(TaskError::HandoffSourceNotAllowed)
            );
        }
        assert_eq!(
            TaskError::HandoffSourceNotAllowed.to_string(),
            "source_session_id is derived from the calling session"
        );
    }

    #[test]
    fn a_review_decision_on_a_revision_is_rejected_by_name() {
        let input = HandoffInput::Revision {
            source_session_id: Some(Uuid::new_v4()),
            commit: SHA1.to_string(),
            comment: "Ready.".to_string(),
            review: Some(ReviewDecision::Approved),
        };

        assert_eq!(
            input.validate(&user_caller()),
            Err(TaskError::HandoffReviewOnRevision)
        );
        assert_eq!(
            TaskError::HandoffReviewOnRevision.to_string(),
            "review applies to forward hand-offs only"
        );
    }

    #[test]
    fn a_revision_commit_must_be_a_full_object_id() {
        for commit in [
            "",
            "0123456",
            &SHA1[..39],
            &SHA1.to_uppercase(),
            "main",
            "HEAD",
            "refs/heads/main",
        ] {
            let input = HandoffInput::Revision {
                source_session_id: Some(Uuid::new_v4()),
                commit: commit.to_string(),
                comment: "Ready.".to_string(),
                review: None,
            };
            assert_eq!(
                input.validate(&user_caller()),
                Err(TaskError::InvalidCommit),
                "accepted {commit:?}"
            );
        }
    }

    #[test]
    fn a_whitespace_only_comment_is_empty_for_both_variants() {
        for comment in ["", "   ", "\t\n "] {
            let revision = HandoffInput::Revision {
                source_session_id: Some(Uuid::new_v4()),
                commit: SHA1.to_string(),
                comment: comment.to_string(),
                review: None,
            };
            assert_eq!(
                revision.validate(&user_caller()),
                Err(TaskError::EmptyComment)
            );

            let forward = HandoffInput::Forward {
                handoff_id: Uuid::new_v4(),
                comment: comment.to_string(),
                review: None,
            };
            assert_eq!(
                forward.validate(&user_caller()),
                Err(TaskError::EmptyComment)
            );
        }
    }

    #[test]
    fn a_forward_keeps_its_hand_off_id_and_optional_decision() {
        let handoff_id = Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap();
        let (session, _) = session_caller();

        for caller in [user_caller(), session] {
            for review in [None, Some(ReviewDecision::ChangesRequested)] {
                let input = HandoffInput::Forward {
                    handoff_id,
                    comment: " Please fix the lint. ".to_string(),
                    review,
                };
                assert_eq!(
                    input.validate(&caller),
                    Ok(ValidatedHandoff::Forward {
                        handoff_id,
                        comment: "Please fix the lint.".to_string(),
                        review,
                    })
                );
            }
        }
    }

    #[test]
    fn a_hand_off_requires_a_move_to_a_different_state() {
        assert_eq!(
            HandoffInput::require_state_change(None, "backlog"),
            Err(TaskError::HandoffRequiresStateChange)
        );
        assert_eq!(
            HandoffInput::require_state_change(Some("backlog"), "backlog"),
            Err(TaskError::HandoffRequiresStateChange)
        );
        assert_eq!(
            HandoffInput::require_state_change(Some("review"), "backlog"),
            Ok(())
        );
        // Exact strings: a differently cased or padded name is a different
        // (and later unknown) state, not the state the task is in.
        assert_eq!(
            HandoffInput::require_state_change(Some("Backlog"), "backlog"),
            Ok(())
        );
        assert_eq!(
            HandoffInput::require_state_change(Some(" backlog"), "backlog"),
            Ok(())
        );
        assert_eq!(
            TaskError::HandoffRequiresStateChange.to_string(),
            "handoff requires a different target state"
        );
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

    #[test]
    fn a_carried_decision_survives_the_reviewer_being_deleted() {
        let reviewed_at = Utc::now();
        let previous = TaskHandoff {
            id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            source_session_id: None,
            source_branch: "session/11111111-1111-4111-8111-111111111111".to_string(),
            commit: SHA1.to_string(),
            comment_id: Some(Uuid::new_v4()),
            review_status: ReviewStatus::Approved,
            // The reviewer's row is gone: `ON DELETE SET NULL` left the
            // verdict and its time behind.
            reviewed_by_user_id: None,
            reviewed_by_session_id: None,
            reviewed_at: Some(reviewed_at),
            created_by_user_id: Some(Uuid::new_v4()),
            created_by_session_id: None,
            created_at: reviewed_at,
        };

        let mut handoff = published();
        handoff.carry_review(&previous);

        assert!(handoff.carried);
        assert_eq!(handoff.review_status, ReviewStatus::Approved);
        assert_eq!(handoff.reviewed_at, Some(reviewed_at));
        assert!(handoff.validate().is_ok());

        // The relaxation is only about the *reviewer*: a carried decision
        // still needs its time, and a carried record may name one reviewer.
        handoff.reviewed_at = None;
        assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));

        handoff.reviewed_at = Some(reviewed_at);
        handoff.reviewed_by_user_id = Some(Uuid::new_v4());
        assert!(handoff.validate().is_ok());
        handoff.reviewed_by_session_id = Some(Uuid::new_v4());
        assert_eq!(handoff.validate(), Err(TaskError::InvalidReview));

        // And it never excuses a record the caller decided itself.
        let mut fresh = published();
        fresh.review_status = ReviewStatus::Approved;
        fresh.reviewed_at = Some(reviewed_at);
        assert_eq!(fresh.validate(), Err(TaskError::InvalidReview));
    }
}
