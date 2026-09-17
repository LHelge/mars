//! Project shared directories: a name on disk and the path every session
//! container mounts it at.
//!
//! `docs/data-model.md`, `project_shared_dirs` is the column contract,
//! `SPEC.md`, "Shared directories" the field contract and `ARCHITECTURE.md`,
//! "Storage" the reason the path rules exist at all: the row becomes a
//! read-write bind mount from `/data/projects/<id>/shared/<name>` into a
//! running container (ADR 0015). A path that is not normalised, that reaches
//! into `/data`, or that shadows the clone, the agent's home, the log
//! directory or the MCP config would mount over the session's own machinery —
//! so the model refuses it here, before a launcher ever hands it to the
//! engine.
//!
//! The one deliberate allowance is a path *inside* `/session/work`, such as
//! `/session/work/target`: that is the expected first use, a nested bind mount
//! over the clone so that every session of a Rust project shares one build
//! directory.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// Longest accepted directory name, in characters (`SPEC.md`, "Shared
/// directories").
pub const MAX_SHARED_DIR_NAME_CHARS: usize = 64;

/// The orchestrator's data volume. Nothing may be mounted at it or below it:
/// that is where the mirror, the CLI state and the shared directories
/// themselves live (`ARCHITECTURE.md`, "Storage").
pub const DATA_SEGMENT: &str = "data";

/// The container paths a shared directory may neither replace nor contain, as
/// their segments (`SPEC.md`, "Shared directories").
///
/// Being an *ancestor* of one of these is what is refused, not being a
/// descendant: `/session` would swallow all four, while `/session/work/target`
/// sits inside the clone and is the intended use.
pub const RESERVED_PATHS: [&[&str]; 4] = [
    &["session", "work"],
    &["session", "home"],
    &["session", "log"],
    &["session", "mcp.json"],
];

/// Every way a shared-directory model can reject its input.
///
/// [`SharedDirError::InvalidPath`] carries the rule that was broken so the 400
/// tells the caller which of the five path rules to fix; the message never
/// echoes the path itself.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SharedDirError {
    /// The name was not 1–64 characters matching `[a-z0-9][a-z0-9_-]*`.
    #[error("shared directory name must be 1-64 characters matching [a-z0-9][a-z0-9_-]*")]
    InvalidName,
    /// The container path broke one of the rules in `SPEC.md`, "Shared
    /// directories"; the payload names which.
    #[error("container path {0}")]
    InvalidPath(&'static str),
}

impl SharedDirError {
    /// The HTTP status this rejection maps to.
    ///
    /// Both variants are malformed input, so both are 400. A name or path that
    /// is well formed but already used in the project is decided against the
    /// table's primary key or unique index and surfaces as [`Error::Conflict`]
    /// from the repository instead (`SPEC.md`, "Shared directories").
    pub fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// The result type the shared-directory models return.
pub type SharedDirResult<T> = std::result::Result<T, SharedDirError>;

/// Does `raw` match the shared-directory name pattern,
/// `[a-z0-9][a-z0-9_-]*` at 1–64 characters?
///
/// Hand-written rather than a regular expression, as everywhere else in the
/// crate. The pattern is deliberately narrower than what a filesystem accepts:
/// the name becomes a directory under `/data/projects/<id>/shared/`, so it may
/// not be `.`, `..`, hidden, or contain a separator, and all three fall out of
/// requiring the first character to be a lower-case letter or digit.
pub fn is_shared_dir_name(raw: &str) -> bool {
    let mut characters = raw.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    if raw.chars().count() > MAX_SHARED_DIR_NAME_CHARS {
        return false;
    }
    characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A validated shared-directory name: 1–64 characters,
/// `[a-z0-9][a-z0-9_-]*`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SharedDirName(String);

impl SharedDirName {
    /// Accept `raw` when it matches the pattern.
    pub fn parse(raw: &str) -> SharedDirResult<Self> {
        if is_shared_dir_name(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(SharedDirError::InvalidName)
        }
    }

    /// The name, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<SharedDirName> for String {
    fn from(name: SharedDirName) -> Self {
        name.0
    }
}

impl std::fmt::Display for SharedDirName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated container mount point: absolute, normalised and outside the
/// reserved paths.
///
/// "Normalised" is checked, never performed: a path that needs normalising is
/// rejected rather than rewritten, so what the user sees in the UI is exactly
/// what the engine is given. The comparison against the reserved list is by
/// segment, not by string prefix, which is why `/session/workspace` is fine
/// while `/session/work` is not.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ContainerPath(String);

impl ContainerPath {
    /// Accept `raw` when it is absolute, has no empty, `.` or `..` segments,
    /// is not `/data` or below it, and neither equals nor contains one of
    /// [`RESERVED_PATHS`].
    pub fn parse(raw: &str) -> SharedDirResult<Self> {
        let segments = Self::segments(raw)?;

        // `/data` is the orchestrator's own volume; mounting anything there
        // would shadow the mirror or the shared directories themselves.
        if segments.first() == Some(&DATA_SEGMENT) {
            return Err(SharedDirError::InvalidPath("must not be /data or below it"));
        }

        // Equal to, or an ancestor of, something the session needs. The empty
        // segment list — `/` — is an ancestor of all four and is caught here.
        for reserved in RESERVED_PATHS {
            if segments.len() <= reserved.len() && reserved[..segments.len()] == segments[..] {
                return Err(SharedDirError::InvalidPath(
                    "must not be, or contain, /session/work, /session/home, /session/log or /session/mcp.json",
                ));
            }
        }

        Ok(Self(raw.to_string()))
    }

    /// The path, ready to bind to the `TEXT` column and to hand to the engine.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The segments of an absolute, normalised `raw`, or the rule it broke.
    ///
    /// `/` yields an empty list, which is what makes the root fall out of the
    /// ancestor check above instead of needing a case of its own.
    fn segments(raw: &str) -> SharedDirResult<Vec<&str>> {
        let Some(rest) = raw.strip_prefix('/') else {
            return Err(SharedDirError::InvalidPath("must be absolute"));
        };
        if rest.is_empty() {
            return Ok(Vec::new());
        }

        let segments: Vec<&str> = rest.split('/').collect();
        for segment in &segments {
            if segment.is_empty() {
                return Err(SharedDirError::InvalidPath(
                    "must not contain repeated or trailing slashes",
                ));
            }
            if *segment == "." || *segment == ".." {
                return Err(SharedDirError::InvalidPath(
                    "must not contain . or .. segments",
                ));
            }
        }

        Ok(segments)
    }
}

impl From<ContainerPath> for String {
    fn from(path: ContainerPath) -> Self {
        path.0
    }
}

impl std::fmt::Display for ContainerPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A `project_shared_dirs` row, column for column (`docs/data-model.md`).
///
/// The API-facing `SharedDir` — `{ name, container_path, created_at }`
/// (`SPEC.md`, "Shared directories") — is assembled by the routes from this
/// row; the project is already in the URL, so the DTO leaves it out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct SharedDir {
    pub project_id: Uuid,
    pub name: String,
    pub container_path: String,
    pub created_at: DateTime<Utc>,
}

/// The caller-supplied half of a new shared directory.
///
/// The project comes from the URL, so it is an argument to the repository
/// rather than a field here (`SPEC.md`, "Shared directories").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSharedDir {
    pub name: SharedDirName,
    pub container_path: ContainerPath,
}

impl NewSharedDir {
    /// Validate both halves of a shared directory.
    pub fn new(name: &str, container_path: &str) -> SharedDirResult<Self> {
        Ok(Self {
            name: SharedDirName::parse(name)?,
            container_path: ContainerPath::parse(container_path)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_is_a_bad_request_with_a_message() {
        for error in [
            SharedDirError::InvalidName,
            SharedDirError::InvalidPath("must be absolute"),
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }
    }

    #[test]
    fn the_crate_error_delegates_to_the_model() {
        let error = Error::from(SharedDirError::InvalidName);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), SharedDirError::InvalidName.to_string());
    }

    #[test]
    fn a_name_accepts_the_documented_pattern() {
        for raw in ["target", "n", "0", "node_modules", "a-b_c9", "9lives"] {
            assert_eq!(SharedDirName::parse(raw).unwrap().as_str(), raw);
            assert!(is_shared_dir_name(raw));
        }
        let longest = "t".repeat(MAX_SHARED_DIR_NAME_CHARS);
        assert_eq!(SharedDirName::parse(&longest).unwrap().as_str(), longest);
    }

    #[test]
    fn a_name_rejects_everything_else() {
        for raw in [
            "", "-target", "_target", ".target", ".", "..", "Target", "tar get", "tar/get", "tär",
            "target!",
        ] {
            assert_eq!(
                SharedDirName::parse(raw),
                Err(SharedDirError::InvalidName),
                "accepted {raw:?}"
            );
            assert!(!is_shared_dir_name(raw), "accepted {raw:?}");
        }
        assert_eq!(
            SharedDirName::parse(&"t".repeat(MAX_SHARED_DIR_NAME_CHARS + 1)),
            Err(SharedDirError::InvalidName)
        );
    }

    #[test]
    fn a_name_renders_as_the_directory_name() {
        let name = SharedDirName::parse("target").unwrap();
        assert_eq!(name.to_string(), "target");
        assert_eq!(String::from(name), "target".to_string());
    }

    #[test]
    fn a_path_accepts_normalised_absolute_paths_outside_the_reserved_set() {
        for raw in [
            "/session/work/target",
            "/session/workspace",
            "/session/work-tree",
            "/cache",
            "/opt/gradle/caches",
            "/session/mcp.json.bak",
            "/datafiles",
        ] {
            assert_eq!(
                ContainerPath::parse(raw).map(|path| path.as_str().to_string()),
                Ok(raw.to_string()),
                "rejected {raw:?}"
            );
        }
    }

    #[test]
    fn a_path_must_not_be_data_or_below_it() {
        for raw in ["/data", "/data/x", "/data/projects/x/shared/target"] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath("must not be /data or below it")),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn a_path_must_not_be_or_contain_a_reserved_path() {
        for raw in [
            "/",
            "/session",
            "/session/work",
            "/session/home",
            "/session/log",
            "/session/mcp.json",
        ] {
            let rejected = ContainerPath::parse(raw).expect_err("accepted a reserved path");
            assert!(
                matches!(rejected, SharedDirError::InvalidPath(rule) if rule.contains("/session/work")),
                "{raw:?} was rejected for the wrong reason: {rejected}"
            );
        }
    }

    #[test]
    fn a_path_must_be_absolute_and_normalised() {
        assert_eq!(
            ContainerPath::parse("relative"),
            Err(SharedDirError::InvalidPath("must be absolute"))
        );
        assert_eq!(
            ContainerPath::parse(""),
            Err(SharedDirError::InvalidPath("must be absolute"))
        );
        for raw in ["/a//b", "/a/", "//a"] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath(
                    "must not contain repeated or trailing slashes"
                )),
                "accepted {raw:?}"
            );
        }
        for raw in ["/a/../b", "/a/./b", "/.", "/..", "/a/.."] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath(
                    "must not contain . or .. segments"
                )),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn a_path_rejection_never_echoes_the_path() {
        let message = ContainerPath::parse("/data/secret-place")
            .unwrap_err()
            .to_string();
        assert!(!message.contains("secret-place"), "leaked: {message}");
    }

    #[test]
    fn a_path_renders_as_the_mount_point() {
        let path = ContainerPath::parse("/session/work/target").unwrap();
        assert_eq!(path.to_string(), "/session/work/target");
        assert_eq!(String::from(path), "/session/work/target".to_string());
    }

    #[test]
    fn a_new_shared_dir_validates_both_halves() {
        let dir = NewSharedDir::new("target", "/session/work/target").unwrap();
        assert_eq!(dir.name.as_str(), "target");
        assert_eq!(dir.container_path.as_str(), "/session/work/target");

        assert_eq!(
            NewSharedDir::new("Target", "/session/work/target").unwrap_err(),
            SharedDirError::InvalidName
        );
        assert!(matches!(
            NewSharedDir::new("target", "/data/target").unwrap_err(),
            SharedDirError::InvalidPath(_)
        ));
    }
}
