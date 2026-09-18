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
//! directory. Nesting is also why the launcher does not take the list in the
//! order it was stored: [`SharedDir::sort_for_mount`] puts parents before the
//! children mounted inside them.

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

/// Longest accepted container path, in bytes.
///
/// Linux's own `PATH_MAX`. A longer path could never be mounted, so it is
/// refused here rather than at the engine, where it would surface as a failed
/// launch of an otherwise healthy session.
pub const MAX_CONTAINER_PATH_BYTES: usize = 4096;

/// The orchestrator's data volume. Nothing may be mounted at it or below it:
/// that is where the mirror, the CLI state and the shared directories
/// themselves live (`ARCHITECTURE.md`, "Storage").
pub const DATA_SEGMENT: &str = "data";

/// The container paths a shared directory may neither replace nor contain, as
/// their segments (`SPEC.md`, "Shared directories").
///
/// Being an *ancestor* of one of these is what is refused, not being a
/// descendant: `/session` would swallow all four, while `/session/work/target`
/// sits inside the clone and is the intended use. The one exception is the
/// fourth, [`MCP_CONFIG_PATH`], which is a file and so has nothing below it.
pub const RESERVED_PATHS: [&[&str]; 4] = [
    &["session", "work"],
    &["session", "home"],
    &["session", "log"],
    MCP_CONFIG_PATH,
];

/// The MCP configuration file's segments.
///
/// The one reserved path that is a file rather than a directory, which is why
/// it needs a name of its own: a mount *below* `/session/work` is the intended
/// use, while a mount below `/session/mcp.json` can never succeed. `SPEC.md`,
/// "Shared directories" spells that out as "or below `/session/mcp.json`".
pub const MCP_CONFIG_PATH: &[&str] = &["session", "mcp.json"];

/// Every way a shared-directory model can reject its input.
///
/// [`SharedDirError::InvalidPath`] carries the rule that was broken so the 400
/// tells the caller which one path rule to fix; the message never echoes the
/// path itself.
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
    /// Accept `raw`, trimmed, when it matches the pattern.
    ///
    /// Surrounding whitespace is a typing artefact of the form field and is
    /// dropped; whitespace anywhere else is not a name character and is
    /// refused, so `" target "` is `target` while `"tar get"` is no name at
    /// all.
    pub fn parse(raw: &str) -> SharedDirResult<Self> {
        let raw = raw.trim();
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
    /// Accept `raw`, trimmed, when it is absolute, has no empty, `.` or `..`
    /// segments and no trailing slash, carries no whitespace or control
    /// characters, is not `/data` or below it, neither equals nor contains one
    /// of [`RESERVED_PATHS`], and is not below [`MCP_CONFIG_PATH`].
    ///
    /// The rules are checked in that order and each has a message of its own,
    /// so the 400 names the one thing to fix. Surrounding whitespace is
    /// trimmed before anything else, which is what makes two paths that differ
    /// only in padding the same path — and therefore the 409 the unique index
    /// promises rather than two rows mounting the same point.
    pub fn parse(raw: &str) -> SharedDirResult<Self> {
        let raw = raw.trim();

        if raw.is_empty() {
            return Err(SharedDirError::InvalidPath("must not be empty"));
        }
        if raw.len() > MAX_CONTAINER_PATH_BYTES {
            return Err(SharedDirError::InvalidPath("must be at most 4096 bytes"));
        }

        let segments = Self::segments(raw)?;

        // A mount point is passed to the engine verbatim; whitespace and
        // control characters in one are far more likely to be a paste
        // accident than a directory anybody meant to share.
        if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(SharedDirError::InvalidPath(
                "must not contain whitespace or control characters",
            ));
        }

        // `/data` is the orchestrator's own volume; mounting anything there
        // would shadow the mirror or the shared directories themselves.
        if segments.first() == Some(&DATA_SEGMENT) {
            return Err(SharedDirError::InvalidPath("must not be /data or below it"));
        }

        // Equal to, or an ancestor of, something the session needs. The empty
        // segment list — `/` — is an ancestor of all four and is caught here.
        for reserved in RESERVED_PATHS {
            if is_ancestor_or_equal(&segments, reserved) {
                return Err(SharedDirError::InvalidPath(
                    "must not be, or contain, /session/work, /session/home, /session/log or /session/mcp.json",
                ));
            }
        }

        // Below the three reserved directories is the intended use; below the
        // reserved *file* is a mount that can never succeed (`SPEC.md`,
        // "Shared directories").
        if segments.len() > MCP_CONFIG_PATH.len()
            && segments[..MCP_CONFIG_PATH.len()] == *MCP_CONFIG_PATH
        {
            return Err(SharedDirError::InvalidPath(
                "must not be below /session/mcp.json, which is a file",
            ));
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
    /// ancestor check above instead of needing a case of its own — and is why
    /// the root is not treated as a trailing slash.
    fn segments(raw: &str) -> SharedDirResult<Vec<&str>> {
        let Some(rest) = raw.strip_prefix('/') else {
            return Err(SharedDirError::InvalidPath("must be absolute"));
        };
        if rest.is_empty() {
            return Ok(Vec::new());
        }
        if rest.ends_with('/') {
            return Err(SharedDirError::InvalidPath("must not end in a slash"));
        }

        let segments: Vec<&str> = rest.split('/').collect();
        for segment in &segments {
            if segment.is_empty() {
                return Err(SharedDirError::InvalidPath(
                    "must not contain repeated slashes",
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

/// Is `candidate` the same path as `reserved`, or an ancestor of it?
///
/// Compared segment by segment, never as strings: `/session/work-tree` shares
/// a string prefix with `/session/work` and is a perfectly good mount point,
/// while `/session` shares no string prefix with `/session/work` at all and is
/// the ancestor that would swallow it. The empty candidate — `/` — is an
/// ancestor of everything, which is exactly what the rule wants.
fn is_ancestor_or_equal(candidate: &[&str], reserved: &[&str]) -> bool {
    candidate.len() <= reserved.len() && reserved[..candidate.len()] == *candidate
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
/// The row serialises as the API-facing `SharedDir` —
/// `{ name, container_path, created_at }` (`SPEC.md`, "Shared directories"):
/// the project is already in the URL of every endpoint that returns one, so
/// `project_id` is kept for the repository and skipped on the way out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct SharedDir {
    #[serde(skip)]
    pub project_id: Uuid,
    pub name: String,
    pub container_path: String,
    pub created_at: DateTime<Utc>,
}

impl SharedDir {
    /// Order `dirs` so that a mount always comes after any mount it nests
    /// inside: by segment count first, then by path.
    ///
    /// A bind mount over a directory that is itself a bind mount only works
    /// when the outer one is in place first (`ARCHITECTURE.md`, "Storage";
    /// the engine tests assert it on both engines). The launcher reads the
    /// list at every launch, so the order lives here rather than being
    /// re-derived at each call site. Sorting by segment count is enough:
    /// a parent has strictly fewer segments than anything below it, and the
    /// path tie-break only keeps the order stable and predictable among
    /// unrelated mounts of the same depth.
    pub fn sort_for_mount(dirs: &mut [SharedDir]) {
        dirs.sort_by(|left, right| {
            depth(&left.container_path)
                .cmp(&depth(&right.container_path))
                .then_with(|| left.container_path.cmp(&right.container_path))
        });
    }
}

/// How many segments an absolute path has.
///
/// The paths here are already validated, so counting separators is the whole
/// job: every segment is preceded by exactly one `/`.
fn depth(container_path: &str) -> usize {
    container_path.matches('/').count()
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
    fn a_name_is_trimmed_before_it_is_judged() {
        assert_eq!(
            SharedDirName::parse("  target\n").unwrap().as_str(),
            "target"
        );
        // Trimming is of the edges only; whitespace inside is not a name.
        assert_eq!(
            SharedDirName::parse("  tar get "),
            Err(SharedDirError::InvalidName)
        );
        assert_eq!(
            SharedDirName::parse("   "),
            Err(SharedDirError::InvalidName)
        );
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

    /// Every entry of the table in `README.md`, "Operating notes", is advice
    /// the product gives; advice the validator refuses would be a bug in one
    /// of the two.
    #[test]
    fn a_path_accepts_every_recommended_entry() {
        for (name, path) in [
            ("target", "/session/work/target"),
            ("cargo-registry", "/session/home/.cargo/registry"),
            ("npm-cache", "/session/home/.npm"),
            ("go-mod", "/session/home/go/pkg/mod"),
            ("go-build", "/session/home/.cache/go-build"),
            ("uv-cache", "/session/home/.cache/uv"),
            ("m2", "/session/home/.m2"),
            ("gradle", "/session/home/.gradle"),
        ] {
            let dir = NewSharedDir::new(name, path)
                .unwrap_or_else(|err| panic!("{name} -> {path} is recommended but refused: {err}"));
            assert_eq!(dir.name.as_str(), name);
            assert_eq!(dir.container_path.as_str(), path);
        }
    }

    #[test]
    fn a_path_is_trimmed_and_bounded() {
        assert_eq!(
            ContainerPath::parse("  /cache\n").unwrap().as_str(),
            "/cache"
        );
        for raw in ["", "   "] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath("must not be empty")),
                "accepted {raw:?}"
            );
        }

        let longest = format!("/{}", "c".repeat(MAX_CONTAINER_PATH_BYTES - 1));
        assert_eq!(ContainerPath::parse(&longest).unwrap().as_str(), longest);
        assert_eq!(
            ContainerPath::parse(&format!("{longest}c")),
            Err(SharedDirError::InvalidPath("must be at most 4096 bytes"))
        );
    }

    #[test]
    fn a_path_must_not_contain_whitespace_or_control_characters() {
        for raw in ["/my cache", "/cache\tdir", "/cache\u{0}dir", "/ca\u{7f}che"] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath(
                    "must not contain whitespace or control characters"
                )),
                "accepted {raw:?}"
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
    fn a_path_may_be_below_a_reserved_directory_but_not_below_the_reserved_file() {
        for raw in [
            "/session/work/target",
            "/session/home/.cargo/registry",
            "/session/log/keep",
        ] {
            assert!(ContainerPath::parse(raw).is_ok(), "rejected {raw:?}");
        }
        for raw in ["/session/mcp.json/x", "/session/mcp.json/x/y"] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath(
                    "must not be below /session/mcp.json, which is a file"
                )),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn an_ancestor_is_compared_segment_by_segment() {
        assert!(is_ancestor_or_equal(&[], &["session", "work"]));
        assert!(is_ancestor_or_equal(&["session"], &["session", "work"]));
        assert!(is_ancestor_or_equal(
            &["session", "work"],
            &["session", "work"]
        ));
        assert!(!is_ancestor_or_equal(
            &["session", "work", "target"],
            &["session", "work"]
        ));
        // A string prefix that is not a segment prefix.
        assert!(!is_ancestor_or_equal(
            &["session", "work-tree"],
            &["session", "work"]
        ));
    }

    #[test]
    fn a_path_must_be_absolute_and_normalised() {
        assert_eq!(
            ContainerPath::parse("relative"),
            Err(SharedDirError::InvalidPath("must be absolute"))
        );
        for raw in ["/a//b", "//a"] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath(
                    "must not contain repeated slashes"
                )),
                "accepted {raw:?}"
            );
        }
        for raw in ["/a/", "/a/b/"] {
            assert_eq!(
                ContainerPath::parse(raw),
                Err(SharedDirError::InvalidPath("must not end in a slash")),
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

    /// A row, as the repository hands one back.
    fn row(name: &str, container_path: &str) -> SharedDir {
        SharedDir {
            project_id: Uuid::nil(),
            name: name.to_string(),
            container_path: container_path.to_string(),
            created_at: DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn mount_order_puts_parents_before_children() {
        let mut dirs = vec![
            row("target", "/session/work/target"),
            row("deep", "/session/work/target/debug/incremental"),
            row("t", "/session/work/t"),
            row("cache", "/cache"),
            row("registry", "/session/home/.cargo/registry"),
        ];
        SharedDir::sort_for_mount(&mut dirs);

        assert_eq!(
            dirs.iter()
                .map(|dir| dir.container_path.as_str())
                .collect::<Vec<_>>(),
            [
                "/cache",
                "/session/work/t",
                "/session/work/target",
                "/session/home/.cargo/registry",
                "/session/work/target/debug/incremental",
            ]
        );
    }

    #[test]
    fn a_row_serialises_as_the_documented_shape() {
        let json = serde_json::to_value(row("target", "/session/work/target")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "target",
                "container_path": "/session/work/target",
                "created_at": "1970-01-01T00:00:00Z",
            })
        );
    }
}
