//! The git shapes the API returns (`SPEC.md`, "Projects": `Branch`).
//!
//! One DTO, deliberately thin: a branch is whatever the project repository's
//! refs currently say, so there is nothing here to validate and no row to map.
//! It lives in `models/` rather than beside a route because three different
//! resources return it — the project's branch list, the session launcher's
//! base picker and the git routes — and `SPEC.md` gives all three the same
//! shape.
//!
//! The three kinds are the three ref namespaces a user may name
//! (`ARCHITECTURE.md`, "Git model", Ref ownership): integration heads under
//! `refs/heads/*`, read-only upstream tracking under
//! `refs/remotes/origin/*` and session work under `refs/sessions/*`. Tags and
//! hand-off refs are deliberately absent: a tag is not a branch, and hand-off
//! refs are internal and are exposed by hand-off id instead (`SPEC.md`,
//! "Git").

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// id.
    Session,
}

/// One ref the API is willing to name (`SPEC.md`, "Projects").
///
/// `name` is the API spelling, which is what every endpoint taking a `source`,
/// `head`, `onto`, `base`, `branch`, `ref` or `target` accepts back: `main`
/// for a head, `origin/main` for upstream tracking, the session id for a
/// session ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    /// The API name, not the fully qualified ref.
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
            name: id.to_string(),
            kind: BranchKind::Session,
            commit: "1".repeat(40),
            session_id: Some(id),
        })
        .expect("a branch serialises");

        assert_eq!(json["kind"], "session");
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
}
