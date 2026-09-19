//! The message a session launched for a task is told before anything else.
//!
//! `SPEC.md`, "Sessions" (`POST /projects/{pid}/sessions` with `task_id`) and
//! `ARCHITECTURE.md`, "Task tracker" → "Launching a session for a task": a
//! session that was launched for a task holds it before it has said a word, so
//! the first thing it reads is which task that is and — when there is code to
//! continue — which revision it was handed.
//!
//! One function, and it is pure: no database, no ids resolved, no truncation.
//! The caller reads the task, its current hand-off and that hand-off's comment
//! under the tracker mutation's lock and hands them here, which is what makes
//! the wording testable without a session, a project or an engine.
//!
//! **Where the text goes** is the caller's too. A conversational session gets
//! it as the first queued input, ahead of the user's own message, so the owner
//! records it as an ordinary `user_message` with no `user_id`; an ephemeral
//! session gets it as the head of the `-p` prompt, because it has no stdin to
//! be told anything on (ADR 0003). Either way this is the text.

use crate::models::{Task, TaskHandoff};
// The crate convention (`CLAUDE.md`, "Backend conventions"); nothing in a pure
// formatter needs the prelude's `Result` or logging.
#[allow(unused_imports)]
use crate::prelude::*;

/// The hand-off a launch starts from, with the body published with it.
///
/// Two values rather than one because the comment is a `task_comments` row the
/// hand-off only points at: it may have been deleted, in which case the
/// paragraph names the revision and stops (`docs/data-model.md`,
/// `task_handoffs`).
#[derive(Debug, Clone, Copy)]
pub struct HandoffContext<'a> {
    /// The record `tasks.current_handoff_id` named under the lock.
    pub handoff: &'a TaskHandoff,
    /// The body of the comment it was published with, when there still is one.
    pub comment: Option<&'a str>,
}

/// The generated first message for a session launched for `task`.
///
/// The first paragraph is always the same sentence, and it is the one
/// `SPEC.md` prints: the task's per-project number and title, and the tool to
/// read the rest with. Nothing is truncated — the agent is given the task's
/// title as it stands, and the description is a `get_task` away.
///
/// A task with a current hand-off adds a second paragraph naming the revision
/// that was handed over: its id, the session and branch it came from, the
/// pinned commit, the review status and the hand-off comment. It is there even
/// when the checkout does *not* start from that commit, because an agent asked
/// to review or continue work needs to know the work exists.
///
/// `base_override` is the explicit `base_ref` the caller gave, and only then:
/// it adds the sentence that says the checkout is somewhere else and names the
/// ref to fetch the hand-off commit from. Without a hand-off there is nothing
/// to override and the argument is ignored.
pub fn generated_task_message(
    task: &Task,
    handoff: Option<HandoffContext<'_>>,
    base_override: Option<&str>,
) -> String {
    let mut text = format!(
        "You hold task #{}: {}. Call get_task to read it before starting.",
        task.number, task.title,
    );

    let Some(HandoffContext { handoff, comment }) = handoff else {
        return text;
    };

    text.push_str("\n\nCurrent hand-off ");
    text.push_str(&handoff.id.to_string());
    // The session may have been deleted since, which nulls the column while
    // the branch and the commit remain; the clause is then simply left out.
    if let Some(source) = handoff.source_session_id {
        text.push_str(&format!(" from session {source}"));
    }
    text.push_str(&format!(
        " on branch {} at commit {} (review: {}).",
        handoff.source_branch, handoff.commit, handoff.review_status,
    ));
    if let Some(comment) = comment {
        text.push_str(&format!(" Hand-off comment: {comment}"));
    }

    if let Some(base) = base_override {
        text.push_str(&format!(
            "\n\nYour checkout starts from {base}, not from the hand-off commit; \
             fetch refs/handoffs/{} before continuing that work.",
            handoff.id,
        ));
    }

    text
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::models::ReviewStatus;

    /// A task carrying `number` and `title` and nothing else worth naming.
    fn task(number: i32, title: &str) -> Task {
        Task {
            id: Uuid::from_u128(1),
            project_id: Uuid::from_u128(2),
            number,
            title: title.to_string(),
            description: String::new(),
            state_id: Uuid::from_u128(3),
            priority: 2,
            blocked: false,
            labels: Vec::new(),
            parent_id: None,
            assignee_user_id: None,
            lease_holder_session_id: None,
            lease_since: None,
            attempts: 0,
            needs_human_reason: None,
            current_handoff_id: None,
            created_by_user_id: None,
            created_by_session_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
        }
    }

    /// A hand-off of `commit` on `branch`, from a session that still exists.
    fn handoff() -> TaskHandoff {
        TaskHandoff {
            id: Uuid::from_u128(10),
            task_id: Uuid::from_u128(1),
            source_session_id: Some(Uuid::from_u128(11)),
            source_branch: "session/11".to_string(),
            commit: "a".repeat(40),
            comment_id: Some(Uuid::from_u128(12)),
            review_status: ReviewStatus::ChangesRequested,
            reviewed_by_user_id: None,
            reviewed_by_session_id: None,
            reviewed_at: None,
            created_by_user_id: None,
            created_by_session_id: Some(Uuid::from_u128(11)),
            created_at: Utc::now(),
        }
    }

    /// Without a hand-off the message is the one documented sentence.
    #[test]
    fn a_task_without_a_handoff_is_one_sentence() {
        let message = generated_task_message(&task(12, "Fix the login form"), None, None);

        assert_eq!(
            message,
            "You hold task #12: Fix the login form. Call get_task to read it before starting.",
        );

        // An explicit base with nothing to override says nothing extra.
        assert_eq!(
            generated_task_message(&task(12, "Fix the login form"), None, Some("main")),
            message,
        );
    }

    /// The hand-off paragraph names the revision, its origin and its comment.
    #[test]
    fn a_handoff_adds_the_revision_paragraph() {
        let handoff = handoff();
        let message = generated_task_message(
            &task(7, "Review the parser"),
            Some(HandoffContext {
                handoff: &handoff,
                comment: Some("Second pass; the lexer still drops comments."),
            }),
            None,
        );

        assert_eq!(
            message,
            format!(
                "You hold task #7: Review the parser. Call get_task to read it before starting.\
                 \n\nCurrent hand-off {} from session {} on branch session/11 at commit {} \
                 (review: changes_requested). Hand-off comment: Second pass; the lexer still \
                 drops comments.",
                handoff.id, "00000000-0000-0000-0000-00000000000b", handoff.commit,
            ),
        );
    }

    /// A deleted source session and a deleted comment each drop their clause.
    #[test]
    fn a_handoff_survives_its_deleted_neighbours() {
        let mut handoff = handoff();
        handoff.source_session_id = None;

        let message = generated_task_message(
            &task(7, "Review the parser"),
            Some(HandoffContext {
                handoff: &handoff,
                comment: None,
            }),
            None,
        );

        assert!(!message.contains("from session"), "{message}");
        assert!(!message.contains("Hand-off comment"), "{message}");
        assert!(
            message.ends_with("(review: changes_requested)."),
            "{message}"
        );
    }

    /// An explicit base over a hand-off says so, and names the ref to fetch.
    #[test]
    fn an_explicit_base_over_a_handoff_is_called_out() {
        let handoff = handoff();
        let message = generated_task_message(
            &task(7, "Review the parser"),
            Some(HandoffContext {
                handoff: &handoff,
                comment: Some("Ready for review."),
            }),
            Some("main"),
        );

        assert!(
            message.ends_with(&format!(
                "\n\nYour checkout starts from main, not from the hand-off commit; \
                 fetch refs/handoffs/{} before continuing that work.",
                handoff.id,
            )),
            "{message}",
        );
    }
}
