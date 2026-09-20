//! The git shapes the API returns (`SPEC.md`, "Projects": `Branch`; "Git":
//! `SessionBranch`, `Diff`).
//!
//! Thin DTOs, deliberately: each one is whatever the project repository's refs
//! and objects currently say, so there is nothing here to persist and no row
//! to map. They live in `models/` rather than beside a route because several
//! resources return them — the project's branch list, the session launcher's
//! base picker, the git routes and the `list_session_branches` MCP tool — and
//! `SPEC.md` gives all of them the same shape.
//!
//! The three kinds are the three ref namespaces a user may name
//! (`ARCHITECTURE.md`, "Git model", Ref ownership): integration heads under
//! `refs/heads/*`, read-only upstream tracking under
//! `refs/remotes/origin/*` and session work under `refs/sessions/*`. Tags and
//! hand-off refs are deliberately absent: a tag is not a branch, and hand-off
//! refs are internal and are exposed by hand-off id instead (`SPEC.md`,
//! "Git").

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The one type a detail borrows from the operation that produces it, rather
// than spelling its three values a second time
// ([`GitRebaseDetail::work_tree`]).
use crate::git::WorkTreeOutcome;
// The crate convention (`CLAUDE.md`, "Backend conventions"); nothing in this
// module needs a prelude item, but every module imports it.
#[allow(unused_imports)]
use crate::prelude::*;

/// Which namespace a [`Branch`] came out of (`SPEC.md`, "Projects").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchKind {
    /// A Mars integration head, `refs/heads/<name>`, named `main`. The only
    /// kind that is both a mutation target and a push source, together with
    /// [`BranchKind::Session`].
    Head,
    /// An upstream-tracking ref, `refs/remotes/origin/<name>`, named
    /// `origin/main`. Read-only: a merge source or a rebase/diff base, never a
    /// target (ADR 0017).
    Upstream,
    /// A session's synced work, `refs/sessions/<session id>`, named by that
    /// full ref.
    Session,
}

/// One ref the API is willing to name (`SPEC.md`, "Projects").
///
/// `name` is a spelling every endpoint taking a `source`, `head`, `onto`,
/// `base`, `branch`, `ref` or `target` accepts back: `main` for a head,
/// `origin/main` for upstream tracking, and the full `refs/sessions/<id>` for
/// a session ref, which says what was selected where a bare id would not
/// (`SPEC.md`, "Projects").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    /// The API name: short for a head or an upstream-tracking ref, fully
    /// qualified for a session ref.
    pub name: String,
    /// Which namespace the ref lives in.
    pub kind: BranchKind,
    /// The full object id the ref points at.
    pub commit: String,
    /// The session this ref belongs to; only ever set for
    /// [`BranchKind::Session`], and omitted from the JSON otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
}

/// How one path changed between two commits (`SPEC.md`, "Git":
/// `Diff.files[].status`).
///
/// The letters are git's own `--name-status` codes and they are what the JSON
/// carries: the "Changes" panel renders a one-letter badge beside each path,
/// so spelling them out here and translating twice would buy nothing.
///
/// [`DiffStatus::Renamed`] and [`DiffStatus::Copied`] are part of the
/// documented shape but are not produced in v1: the diff is taken with rename
/// detection off, so that the status list and the line counts describe exactly
/// the same set of paths ([`crate::git::diff`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffStatus {
    /// `A`: the path did not exist at the merge base.
    #[serde(rename = "A")]
    Added,
    /// `M`: the path exists in both, and its content or its mode changed.
    #[serde(rename = "M")]
    Modified,
    /// `D`: the path existed at the merge base and is gone.
    #[serde(rename = "D")]
    Deleted,
    /// `R`: the path was renamed from another one.
    #[serde(rename = "R")]
    Renamed,
    /// `C`: the path was copied from another one.
    #[serde(rename = "C")]
    Copied,
    /// `T`: the entry changed type — a file that became a symlink, say.
    #[serde(rename = "T")]
    TypeChanged,
}

impl DiffStatus {
    /// The status one of git's letters names, or `None` for anything else.
    ///
    /// `U` (unmerged) and `X` (git's own "this should not happen") are the
    /// deliberate `None` cases: neither can occur in a diff between two
    /// commits, and inventing a status for one would report a change that was
    /// never described.
    pub fn parse(letter: char) -> Option<Self> {
        match letter {
            'A' => Some(DiffStatus::Added),
            'M' => Some(DiffStatus::Modified),
            'D' => Some(DiffStatus::Deleted),
            'R' => Some(DiffStatus::Renamed),
            'C' => Some(DiffStatus::Copied),
            'T' => Some(DiffStatus::TypeChanged),
            _ => None,
        }
    }

    /// The letter this status serialises as.
    pub fn letter(self) -> char {
        match self {
            DiffStatus::Added => 'A',
            DiffStatus::Modified => 'M',
            DiffStatus::Deleted => 'D',
            DiffStatus::Renamed => 'R',
            DiffStatus::Copied => 'C',
            DiffStatus::TypeChanged => 'T',
        }
    }
}

/// One changed path in a [`Diff`] (`SPEC.md`, "Git").
///
/// `additions` and `deletions` are git's `--numstat` counts. A binary file has
/// none — git prints `-` for both — and reports zeroes, which is what the
/// panel shows beside the file rather than a missing count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    /// The path as git named it, relative to the repository root.
    pub path: String,
    /// How it changed.
    pub status: DiffStatus,
    /// Lines added, or 0 for a binary file.
    pub additions: u32,
    /// Lines removed, or 0 for a binary file.
    pub deletions: u32,
}

/// What `GET /projects/{pid}/git/diff` returns (`SPEC.md`, "Git";
/// `ARCHITECTURE.md`, "Git model", Diff).
///
/// The comparison runs from `merge_base(base, head)` to `head`, never from
/// `base` itself: the "Changes" panel has to keep showing a session's own work
/// after the integration branch it started from has moved on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    /// The API name of the base that was compared against.
    pub base: String,
    /// The API name of the head that was compared.
    pub head: String,
    /// The full object id the patch starts from.
    pub merge_base: String,
    /// The changed paths, in git's own order.
    pub files: Vec<DiffFile>,
    /// The unified patch, cut at the caller's byte limit when `truncated`.
    pub patch: String,
    /// Whether `patch` is only the beginning of the real one.
    pub truncated: bool,
}

/// One session's work branch and how it stands against the project's
/// integration head (`SPEC.md`, "Git"; MCP `list_session_branches`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBranch {
    /// The session whose work this is.
    pub session_id: Uuid,
    /// The fully qualified ref, `refs/sessions/<session id>`. Spelled out
    /// rather than left implicit, because it is what the agent-facing tool
    /// reports.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The full object id the ref points at.
    pub commit: String,
    /// Commits on this branch that `base` does not have.
    pub ahead: u32,
    /// Commits on `base` that this branch does not have.
    pub behind: u32,
    /// The integration head the counts are measured against: the project's
    /// `default_branch`, by its short name.
    pub base: String,
    /// The committer date of the branch's tip, which is what the list is
    /// ordered by.
    pub updated_at: DateTime<Utc>,
}

/// What `POST /sessions/{id}/sync` answers, and what a successful `sync`
/// event's `detail` carries (`SPEC.md`, "Sessions"; "AgentEvent").
///
/// `ref` is the fully qualified `refs/sessions/<session id>`, the same
/// spelling [`SessionBranch::git_ref`] uses, so the two describe one ref the
/// same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOutcome {
    /// The session ref that was updated.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The commit it now points at.
    pub commit: String,
}

/// The `detail` of a `git` event with `op: "sync"` (`SPEC.md`, "AgentEvent").
///
/// `commit` on success, `error` on failure; never both. `error` is the generic
/// user-facing message [`Error::user_message`] produces — never git's stderr
/// and never a credential (`CLAUDE.md` rule 3), which is the rule for the
/// `error` field of every detail in this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSyncDetail {
    /// The session ref the sync was for.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The commit the ref now points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Why the sync failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `detail` of a `git` event with `op: "merge"` (`SPEC.md`, "AgentEvent").
///
/// `source` and `target` are API ref names, except for the task-merge form,
/// where `source` is the label the caller gave the pinned hand-off commit.
/// `requested_by` is `user:<uuid>`, `session:<uuid>` or `system`, the same
/// value the merge commit's `Requested-By` trailer carries
/// (`ARCHITECTURE.md`, "Git model", Commit identity).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitMergeDetail {
    /// What was merged.
    pub source: String,
    /// The integration head it was merged into.
    pub target: String,
    /// The commit the target now points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Whether the target moved to the source commit rather than gaining a
    /// merge commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_forward: Option<bool>,
    /// The paths a stopped merge left conflicting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<String>>,
    /// Who asked for the operation.
    pub requested_by: String,
    /// Why the merge failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `detail` of a `git` event with `op: "rebase"` (`SPEC.md`,
/// "AgentEvent").
///
/// `work_tree` is the one thing only this event reports: whether the session's
/// checkout was updated to the rewritten commits or still has to be reconciled
/// by somebody (`ARCHITECTURE.md`, "Git model", Merge, rebase, push).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitRebaseDetail {
    /// The branch that was rewritten.
    pub branch: String,
    /// What it was replayed onto.
    pub onto: String,
    /// The commit the branch now points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The paths a stopped rebase left conflicting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<String>>,
    /// What became of the session's checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_tree: Option<WorkTreeOutcome>,
    /// Who asked for the operation.
    pub requested_by: String,
    /// Why the rebase failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `detail` of a `git` event with `op: "push"` (`SPEC.md`, "AgentEvent").
///
/// `compare_url` is where the UI offers "open a pull request"; it is absent
/// for a remote that is not a GitHub one. The REST body stays
/// `{remote_branch, commit}` (`SPEC.md`, "Git"), which is why the link travels
/// here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPushDetail {
    /// The local ref that was pushed, by its API name.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The upstream branch it was sent to, short name.
    pub remote_branch: String,
    /// The commit that was published.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Whether the caller asked for a forced push.
    pub force: bool,
    /// The GitHub compare page for opening a pull request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compare_url: Option<String>,
    /// Who asked for the operation.
    pub requested_by: String,
    /// Why the push failed; a non-fast-forward rejection says so here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_serialises_without_a_session_id() {
        let json = serde_json::to_value(Branch {
            name: "main".to_string(),
            kind: BranchKind::Head,
            commit: "0".repeat(40),
            session_id: None,
        })
        .expect("a branch serialises");

        assert_eq!(json["name"], "main");
        assert_eq!(json["kind"], "head");
        assert!(json.get("session_id").is_none(), "{json}");
    }

    #[test]
    fn a_session_branch_carries_its_id_and_the_snake_case_kind() {
        let id = Uuid::new_v4();
        let json = serde_json::to_value(Branch {
            name: format!("refs/sessions/{id}"),
            kind: BranchKind::Session,
            commit: "1".repeat(40),
            session_id: Some(id),
        })
        .expect("a branch serialises");

        assert_eq!(json["kind"], "session");
        assert_eq!(json["name"], format!("refs/sessions/{id}"));
        assert_eq!(json["session_id"], id.to_string());
    }

    #[test]
    fn upstream_is_the_documented_spelling() {
        let json = serde_json::to_value(Branch {
            name: "origin/main".to_string(),
            kind: BranchKind::Upstream,
            commit: "2".repeat(40),
            session_id: None,
        })
        .expect("a branch serialises");

        assert_eq!(json["kind"], "upstream");
        assert_eq!(json["name"], "origin/main");
    }

    #[test]
    fn every_diff_status_serialises_as_the_letter_git_printed() {
        for status in [
            DiffStatus::Added,
            DiffStatus::Modified,
            DiffStatus::Deleted,
            DiffStatus::Renamed,
            DiffStatus::Copied,
            DiffStatus::TypeChanged,
        ] {
            let json = serde_json::to_value(status).expect("a status serialises");

            assert_eq!(json, serde_json::json!(status.letter().to_string()));
            assert_eq!(DiffStatus::parse(status.letter()), Some(status));
        }
    }

    #[test]
    fn a_letter_that_cannot_occur_between_two_commits_is_not_a_status() {
        for letter in ['U', 'X', 'a', ' ', '?'] {
            assert_eq!(DiffStatus::parse(letter), None, "{letter} parsed");
        }
    }

    #[test]
    fn a_diff_serialises_the_documented_shape() {
        let json = serde_json::to_value(Diff {
            base: "main".to_string(),
            head: "origin/main".to_string(),
            merge_base: "3".repeat(40),
            files: vec![DiffFile {
                path: "src/lib.rs".to_string(),
                status: DiffStatus::Modified,
                additions: 12,
                deletions: 3,
            }],
            patch: "diff --git a/src/lib.rs b/src/lib.rs\n".to_string(),
            truncated: false,
        })
        .expect("a diff serialises");

        assert_eq!(json["base"], "main");
        assert_eq!(json["head"], "origin/main");
        assert_eq!(json["merge_base"], "3".repeat(40));
        assert_eq!(json["files"][0]["path"], "src/lib.rs");
        assert_eq!(json["files"][0]["status"], "M");
        assert_eq!(json["files"][0]["additions"], 12);
        assert_eq!(json["files"][0]["deletions"], 3);
        assert_eq!(json["truncated"], false);
    }

    #[test]
    fn a_session_branch_spells_its_ref_field_ref() {
        let session_id = Uuid::new_v4();
        let json = serde_json::to_value(SessionBranch {
            session_id,
            git_ref: format!("refs/sessions/{session_id}"),
            commit: "4".repeat(40),
            ahead: 2,
            behind: 1,
            base: "main".to_string(),
            updated_at: DateTime::parse_from_rfc3339("2025-01-02T03:04:05Z")
                .expect("a fixed timestamp")
                .with_timezone(&Utc),
        })
        .expect("a session branch serialises");

        assert_eq!(json["session_id"], session_id.to_string());
        assert_eq!(json["ref"], format!("refs/sessions/{session_id}"));
        assert!(json.get("git_ref").is_none(), "{json}");
        assert_eq!(json["ahead"], 2);
        assert_eq!(json["behind"], 1);
        assert_eq!(json["base"], "main");
        assert_eq!(json["updated_at"], "2025-01-02T03:04:05Z");
    }

    #[test]
    fn a_successful_detail_carries_no_error_and_a_failed_one_no_commit() {
        let succeeded = serde_json::to_value(GitSyncDetail {
            git_ref: "refs/sessions/x".to_string(),
            commit: Some("5".repeat(40)),
            error: None,
        })
        .expect("a detail serialises");
        assert_eq!(succeeded["ref"], "refs/sessions/x");
        assert_eq!(succeeded["commit"], "5".repeat(40));
        assert!(succeeded.get("error").is_none(), "{succeeded}");

        let failed = serde_json::to_value(GitSyncDetail {
            git_ref: "refs/sessions/x".to_string(),
            commit: None,
            error: Some("internal error".to_string()),
        })
        .expect("a detail serialises");
        assert!(failed.get("commit").is_none(), "{failed}");
        assert_eq!(failed["error"], "internal error");
    }

    #[test]
    fn a_merge_detail_carries_its_outcome_and_who_asked() {
        let user_id = Uuid::new_v4();
        let json = serde_json::to_value(GitMergeDetail {
            source: "main".to_string(),
            target: "release".to_string(),
            commit: Some("6".repeat(40)),
            fast_forward: Some(false),
            conflicts: None,
            requested_by: format!("user:{user_id}"),
            error: None,
        })
        .expect("a detail serialises");

        assert_eq!(json["source"], "main");
        assert_eq!(json["target"], "release");
        assert_eq!(json["fast_forward"], false);
        assert_eq!(json["requested_by"], format!("user:{user_id}"));
        assert!(json.get("conflicts").is_none(), "{json}");
    }

    #[test]
    fn a_rebase_detail_spells_the_work_tree_outcome_as_the_event_contract_does() {
        let json = serde_json::to_value(GitRebaseDetail {
            branch: "main".to_string(),
            onto: "origin/main".to_string(),
            commit: None,
            conflicts: Some(vec!["src/lib.rs".to_string()]),
            work_tree: Some(WorkTreeOutcome::ReconciliationRequired),
            requested_by: "system".to_string(),
            error: Some("merge conflict".to_string()),
        })
        .expect("a detail serialises");

        assert_eq!(json["work_tree"], "reconciliation_required");
        assert_eq!(json["conflicts"][0], "src/lib.rs");
        assert_eq!(json["error"], "merge conflict");
    }

    #[test]
    fn a_push_detail_spells_its_ref_field_ref_and_always_says_whether_it_forced() {
        let json = serde_json::to_value(GitPushDetail {
            git_ref: "main".to_string(),
            remote_branch: "main".to_string(),
            commit: Some("7".repeat(40)),
            force: false,
            compare_url: None,
            requested_by: "system".to_string(),
            error: None,
        })
        .expect("a detail serialises");

        assert_eq!(json["ref"], "main");
        assert!(json.get("git_ref").is_none(), "{json}");
        assert_eq!(json["force"], false);
        assert!(json.get("compare_url").is_none(), "{json}");
    }

    #[test]
    fn a_sync_outcome_is_the_documented_response_body() {
        let json = serde_json::to_value(SyncOutcome {
            git_ref: "refs/sessions/x".to_string(),
            commit: "8".repeat(40),
        })
        .expect("an outcome serialises");

        assert_eq!(json["ref"], "refs/sessions/x");
        assert_eq!(json["commit"], "8".repeat(40));
    }
}
