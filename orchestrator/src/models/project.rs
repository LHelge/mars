//! Projects: one git repository, its clone status and the two counters the
//! tracker depends on.
//!
//! `docs/data-model.md`, `projects` is the column contract and `SPEC.md`,
//! "Projects" is the field contract the API exposes. Validation lives here,
//! SQL lives in `repositories/projects.rs` (`CLAUDE.md`, "Backend
//! conventions").
//!
//! Two fields carry more weight than their types suggest. `remote_url` is
//! `https://` only and must never carry userinfo, because the credential is a
//! project-scoped secret named `GIT_CREDENTIAL` and a URL that smuggles one
//! past that mechanism would end up in logs, events and the API (`CLAUDE.md`,
//! rule 3). `default_branch` may be null while discovery from the remote
//! `HEAD` is pending or has failed, but a `ready` project always has one — a
//! table `CHECK` enforces that, and the repository turns the violation into a
//! conflict rather than a 500.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions"). Models report
// their own error rather than the crate-wide one, so the glob is here for the
// doc links and for what these models grow into.
#[allow(unused_imports)]
use crate::prelude::*;

/// Longest accepted project name, in characters (`docs/data-model.md`,
/// `projects`).
pub const MAX_PROJECT_NAME_CHARS: usize = 100;

/// The only accepted remote scheme in v1 (`SPEC.md`, "Projects").
pub const REMOTE_URL_SCHEME: &str = "https://";

/// The extra remote scheme integration tests may use; see
/// [`RemoteUrl::parse`].
#[cfg(feature = "integration-tests")]
pub const LOCAL_REMOTE_SCHEME: &str = "file://";

/// Longest accepted branch name, in characters.
///
/// Git itself bounds a ref by the filesystem rather than by a number; 255 is
/// the longest single path component every supported filesystem stores, so a
/// name longer than this could not become `refs/heads/<name>` on disk anyway.
pub const MAX_BRANCH_NAME_CHARS: usize = 255;

/// Fewest claims a task may go through in one state (`docs/data-model.md`,
/// `projects`).
pub const MIN_MAX_ATTEMPTS: i16 = 1;

/// Most claims a task may go through in one state.
pub const MAX_MAX_ATTEMPTS: i16 = 20;

/// The column default, repeated here so callers that build a [`NewProject`]
/// without an explicit value use the documented number.
pub const DEFAULT_MAX_ATTEMPTS: i16 = 3;

/// Where a project is in its clone lifecycle (`docs/data-model.md`, "Enums").
///
/// `Cloning` is set at creation; the clone job moves it to `Ready` or `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "project_status", rename_all = "snake_case")]
pub enum ProjectStatus {
    /// The mirror is being created; no session can start yet.
    Cloning,
    /// The mirror exists and `default_branch` resolves to an integration head.
    Ready,
    /// The clone or a later fetch failed; `status_message` says why.
    Error,
}

/// Every way a project model can reject its input.
///
/// `Display` is the message the API returns in `{ status, error }`, so each
/// variant says what the caller has to change and never echoes the rejected
/// value — a `remote_url` is exactly the field a mistyped credential would
/// hide in (`CLAUDE.md`, rule 3). [`ProjectError::status`] is the HTTP status
/// the crate-wide [`Error`] delegates to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectError {
    /// The name was not 1–100 characters after trimming.
    #[error("project name must be 1-100 characters")]
    InvalidName,
    /// The remote was not a well-formed `https://` URL.
    #[error("remote URL must be an https:// URL without embedded credentials")]
    InvalidRemoteUrl,
    /// The remote carried userinfo — a credential in a column that is read
    /// back by `GET /projects` (`CLAUDE.md`, rule 3).
    ///
    /// A separate variant from [`ProjectError::InvalidRemoteUrl`] because the
    /// caller has to do something different about it: move the value into the
    /// `credential` field, which is stored as the project-scoped
    /// `GIT_CREDENTIAL` secret.
    #[error("remote_url must not contain credentials")]
    RemoteUrlHasCredentials,
    /// The branch name was empty, too long, or not a name git would accept.
    #[error("default branch must be a valid git branch name")]
    InvalidDefaultBranch,
    /// `max_attempts` was outside 1–20.
    #[error("max attempts must be between 1 and 20")]
    InvalidMaxAttempts,
}

impl ProjectError {
    /// The HTTP status this rejection maps to.
    ///
    /// Every variant is malformed input, so every variant is 400. A name that
    /// is well formed but already taken is decided against `projects_name_key`
    /// and surfaces as [`Error::Conflict`] from the repository instead
    /// (`SPEC.md`, "Projects").
    pub fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// The result type the project models return.
pub type ProjectResult<T> = std::result::Result<T, ProjectError>;

/// A validated project name: trimmed, 1–100 characters.
///
/// Case and interior spacing are preserved: this is a display name, and
/// `projects_name_key` compares the stored bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProjectName(String);

impl ProjectName {
    /// Trim `raw` and accept it when 1–100 characters remain.
    pub fn parse(raw: &str) -> ProjectResult<Self> {
        let trimmed = raw.trim();
        let length = trimmed.chars().count();
        if length == 0 || length > MAX_PROJECT_NAME_CHARS {
            return Err(ProjectError::InvalidName);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The trimmed name, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<ProjectName> for String {
    fn from(name: ProjectName) -> Self {
        name.0
    }
}

impl std::fmt::Display for ProjectName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated remote URL: `https://`, no userinfo, no whitespace.
///
/// Stored exactly as given otherwise. A trailing `.git` is neither added nor
/// removed, because the remote decides which form it serves and normalising
/// would make two rows that clone the same repository look different from the
/// one the user typed.
///
/// The userinfo rule is the important one: `https://user:token@host/repo.git`
/// is a credential in a column that is returned by `GET /projects`, logged by
/// git and copied into events. The credential belongs in the project-scoped
/// `GIT_CREDENTIAL` secret, so a URL that carries one is rejected outright
/// (`CLAUDE.md`, rule 3; `SPEC.md`, "Projects").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RemoteUrl(String);

impl RemoteUrl {
    /// Trim `raw`, then accept it when it is an `https://` URL with a
    /// non-empty authority that carries no userinfo, no fragment and no
    /// whitespace or control character anywhere.
    ///
    /// **Test-only exception.** Under the `integration-tests` feature an
    /// absolute `file://` URL is accepted as well, so that the integration
    /// suite can point a project at a real bare repository in a `tempfile`
    /// directory and clone it for real — git is never mocked (`CLAUDE.md`,
    /// "Testing expectations"). The release build has no such branch and keeps
    /// `https://` only, which is the documented contract (`SPEC.md`,
    /// "Projects"; `docs/data-model.md`, `projects`). This is deliberately not
    /// in `SPEC.md`: the behaviour a released Mars has is unchanged.
    pub fn parse(raw: &str) -> ProjectResult<Self> {
        let trimmed = raw.trim();

        // Interior whitespace survives the trim and would reach `git` as part
        // of the argument; a control character would reach a log line. A
        // fragment is never part of a git remote, and `#` in a URL that is
        // also written into `.git/config` is asking for trouble.
        if trimmed
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
            || trimmed.contains('#')
        {
            return Err(ProjectError::InvalidRemoteUrl);
        }

        if let Some(rest) = trimmed.strip_prefix(REMOTE_URL_SCHEME) {
            // The authority is everything up to the first `/` or `?` after the
            // scheme; a `@` there is userinfo. A `@` later in the path is an
            // ordinary character (GitLab subgroups and Gitea mirrors both
            // produce some).
            let authority = rest.split(['/', '?']).next().unwrap_or(rest);
            if authority.contains('@') {
                return Err(ProjectError::RemoteUrlHasCredentials);
            }
            if authority.is_empty() {
                return Err(ProjectError::InvalidRemoteUrl);
            }
        } else if !is_local_test_remote(trimmed) {
            return Err(ProjectError::InvalidRemoteUrl);
        }

        Ok(Self(trimmed.to_string()))
    }

    /// The URL, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A local path standing in for a remote, for tests only.
    ///
    /// Git treats a path as a remote over the `file` transport, which is what
    /// lets the git tests point `origin` at a bare repository in a `tempfile`
    /// directory instead of at a network host (`CLAUDE.md`, "Testing
    /// expectations": git is never mocked). A bare path has no scheme, so
    /// [`RemoteUrl::parse`] rejects it however it is built — the test-only
    /// form a *request* can carry is the `file://` URL that parse accepts
    /// under the same feature — and this bypass is compiled only under
    /// `cfg(test)` or the `integration-tests` feature, so no request reaches
    /// it at all.
    #[cfg(any(test, feature = "integration-tests"))]
    pub fn local_for_tests(path: &std::path::Path) -> Self {
        Self(path.display().to_string())
    }
}

/// Is `trimmed` the test-only local remote form, `file://` plus an absolute
/// path?
///
/// Compiled under the `integration-tests` feature only; the counterpart below
/// is what a release build gets, and it accepts nothing (see
/// [`RemoteUrl::parse`]).
#[cfg(feature = "integration-tests")]
fn is_local_test_remote(trimmed: &str) -> bool {
    matches!(
        trimmed.strip_prefix(LOCAL_REMOTE_SCHEME),
        Some(path) if path.starts_with('/')
    )
}

/// A release build accepts no scheme but `https://`.
#[cfg(not(feature = "integration-tests"))]
fn is_local_test_remote(_trimmed: &str) -> bool {
    false
}

impl From<RemoteUrl> for String {
    fn from(url: RemoteUrl) -> Self {
        url.0
    }
}

impl std::fmt::Display for RemoteUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The characters git's own `check-ref-format` never allows in a ref
/// component, minus the ones spelled out separately below.
const BRANCH_NAME_FORBIDDEN: [char; 7] = ['~', '^', ':', '?', '*', '[', '\\'];

/// Does `raw` look like a branch name Mars will accept?
///
/// A conservative subset of `git check-ref-format`: 1–255 characters, no
/// whitespace and no control characters, none of `~ ^ : ? * [ \`, no `..` and
/// no `@{`, not starting with `-`, `/` or `refs/` and not ending with `/`, `.`
/// or `.lock`. Everything on that list either cannot become a ref at all, or
/// would be read as an option or a revision expression by the `git` binary the
/// name is passed to (ADR 0011).
///
/// It is deliberately a *syntactic* check. Whether the branch exists is a
/// question for the mirror, and the clone job answers it before the project
/// becomes `ready`.
pub fn is_branch_name(raw: &str) -> bool {
    let length = raw.chars().count();
    if length == 0 || length > MAX_BRANCH_NAME_CHARS {
        return false;
    }
    if raw
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control() || BRANCH_NAME_FORBIDDEN.contains(&ch))
    {
        return false;
    }
    if raw.contains("..") || raw.contains("@{") {
        return false;
    }
    if raw.starts_with('-') || raw.starts_with('/') || raw.starts_with("refs/") {
        return false;
    }
    if raw.ends_with('/') || raw.ends_with('.') || raw.ends_with(".lock") {
        return false;
    }

    true
}

/// A validated branch name: see [`is_branch_name`] for the rule set.
///
/// Optional on a project — `default_branch` is null while discovery from the
/// remote `HEAD` is pending or has failed — but never a bare `String` when it
/// is present, so nothing can store a branch the launcher could not resolve.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BranchName(String);

impl BranchName {
    /// Accept `raw` when [`is_branch_name`] does.
    pub fn parse(raw: &str) -> ProjectResult<Self> {
        if is_branch_name(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(ProjectError::InvalidDefaultBranch)
        }
    }

    /// The branch name, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<BranchName> for String {
    fn from(branch: BranchName) -> Self {
        branch.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated attempt budget: 1–20 (`SPEC.md`, "Projects").
///
/// How many times a task may be claimed in one state before a release sends it
/// to the project's human state instead. The same bounds are a table `CHECK`,
/// so this type is what keeps a bad value from reaching the database as a 500
/// instead of a 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct MaxAttempts(i16);

impl MaxAttempts {
    /// Accept `raw` when it is between 1 and 20.
    pub fn parse(raw: i16) -> ProjectResult<Self> {
        if (MIN_MAX_ATTEMPTS..=MAX_MAX_ATTEMPTS).contains(&raw) {
            Ok(Self(raw))
        } else {
            Err(ProjectError::InvalidMaxAttempts)
        }
    }

    /// The number, ready to bind to the `SMALLINT` column.
    pub fn get(self) -> i16 {
        self.0
    }
}

impl Default for MaxAttempts {
    /// The documented default of 3.
    fn default() -> Self {
        Self(DEFAULT_MAX_ATTEMPTS)
    }
}

impl std::fmt::Display for MaxAttempts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A `projects` row, column for column (`docs/data-model.md`, `projects`).
///
/// The API-facing `Project` — the same fields without `created_by`,
/// `next_task_number` and `updated_at` (`SPEC.md`, "Projects") — is assembled
/// by the routes from this row.
///
/// `has_credential` is not a column: it is whether the project has the
/// project-scoped `GIT_CREDENTIAL` secret, which lives in `secrets`. Every
/// read in [`crate::repositories::ProjectRepository`] computes it with an
/// `EXISTS` subquery in the same statement, so a project is rendered from one
/// round trip and no ciphertext is moved to answer a boolean
/// (`docs/data-model.md`, `projects`, `secrets`).
///
/// There is no `Deserialize`: a `Project` only ever comes out of the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub remote_url: String,
    pub default_branch: Option<String>,
    pub status: ProjectStatus,
    pub status_message: Option<String>,
    pub created_by: Option<Uuid>,
    pub last_fetched_at: Option<DateTime<Utc>>,
    pub max_attempts: i16,
    pub next_task_number: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Derived, not stored; see the type documentation. Last, because
    /// `query_as!` maps columns to fields by position.
    pub has_credential: bool,
}

/// The caller-supplied half of a new project.
///
/// The id is generated up front because it is also the directory name under
/// `/data/projects/`, so the clone job needs it before the row exists. The
/// repository leaves `status` and `next_task_number` to their column defaults:
/// a project starts `cloning` with task numbering at 1, and only the clone job
/// moves it on.
///
/// The credential the caller may have supplied is not here. It is stored as
/// the project-scoped, orchestrator-only secret `GIT_CREDENTIAL` by the
/// projects epic and never travels with the row (`SPEC.md`, "Projects").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject {
    pub id: Uuid,
    pub name: ProjectName,
    pub remote_url: RemoteUrl,
    pub default_branch: Option<BranchName>,
    pub created_by: Option<Uuid>,
    pub max_attempts: MaxAttempts,
}

impl NewProject {
    /// A project with a fresh id, no default branch and the default attempt
    /// budget, validating the name and the remote.
    ///
    /// The remaining fields are public: callers set what they were given.
    pub fn new(name: &str, remote_url: &str) -> ProjectResult<Self> {
        Ok(Self {
            id: Uuid::new_v4(),
            name: ProjectName::parse(name)?,
            remote_url: RemoteUrl::parse(remote_url)?,
            default_branch: None,
            created_by: None,
            max_attempts: MaxAttempts::default(),
        })
    }
}

/// The fields `PUT /projects/{id}` can change (`SPEC.md`, "Projects").
///
/// Every field is optional and `None` means "leave it alone", so one statement
/// serves the whole endpoint. `status`, `status_message` and `last_fetched_at`
/// are deliberately absent: they are the clone and fetch jobs' to set, through
/// [`crate::repositories::ProjectRepository::mark_ready`],
/// [`crate::repositories::ProjectRepository::mark_error`],
/// [`crate::repositories::ProjectRepository::mark_cloning_from_error`],
/// [`crate::repositories::ProjectRepository::set_status`] and
/// [`crate::repositories::ProjectRepository::set_last_fetched_at`], never
/// through a user-supplied body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectUpdate {
    pub name: Option<ProjectName>,
    pub default_branch: Option<BranchName>,
    pub max_attempts: Option<MaxAttempts>,
}

impl ProjectUpdate {
    /// Whether this update would change anything at all.
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.default_branch.is_none() && self.max_attempts.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_is_a_bad_request_with_a_message() {
        for error in [
            ProjectError::InvalidName,
            ProjectError::InvalidRemoteUrl,
            ProjectError::RemoteUrlHasCredentials,
            ProjectError::InvalidDefaultBranch,
            ProjectError::InvalidMaxAttempts,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }
    }

    #[test]
    fn the_crate_error_delegates_to_the_model() {
        let error = Error::from(ProjectError::InvalidRemoteUrl);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            error.to_string(),
            ProjectError::InvalidRemoteUrl.to_string()
        );
    }

    #[test]
    fn the_status_enum_serialises_in_snake_case() {
        assert_eq!(
            serde_json::to_value(ProjectStatus::Cloning).unwrap(),
            serde_json::json!("cloning")
        );
        assert_eq!(
            serde_json::to_value(ProjectStatus::Ready).unwrap(),
            serde_json::json!("ready")
        );
        assert_eq!(
            serde_json::from_value::<ProjectStatus>(serde_json::json!("error")).unwrap(),
            ProjectStatus::Error
        );
    }

    #[test]
    fn a_name_is_trimmed() {
        let name = ProjectName::parse("  mars \n").unwrap();
        assert_eq!(name.as_str(), "mars");
        assert_eq!(name.to_string(), "mars");
        assert_eq!(String::from(name), "mars");
    }

    #[test]
    fn a_name_accepts_one_and_a_hundred_characters() {
        assert_eq!(ProjectName::parse("m").unwrap().as_str(), "m");
        let longest = "m".repeat(MAX_PROJECT_NAME_CHARS);
        assert_eq!(ProjectName::parse(&longest).unwrap().as_str(), longest);
    }

    #[test]
    fn a_name_rejects_empty_and_a_hundred_and_one_characters() {
        assert_eq!(ProjectName::parse(""), Err(ProjectError::InvalidName));
        assert_eq!(ProjectName::parse("   \t "), Err(ProjectError::InvalidName));
        assert_eq!(
            ProjectName::parse(&"m".repeat(MAX_PROJECT_NAME_CHARS + 1)),
            Err(ProjectError::InvalidName)
        );
    }

    #[test]
    fn a_name_counts_characters_not_bytes() {
        assert!(ProjectName::parse(&"å".repeat(MAX_PROJECT_NAME_CHARS)).is_ok());
        assert_eq!(
            ProjectName::parse(&"å".repeat(MAX_PROJECT_NAME_CHARS + 1)),
            Err(ProjectError::InvalidName)
        );
    }

    #[test]
    fn a_remote_url_keeps_what_it_was_given() {
        // With and without the suffix, both stored as typed.
        assert_eq!(
            RemoteUrl::parse("https://example.invalid/org/repo.git")
                .unwrap()
                .as_str(),
            "https://example.invalid/org/repo.git"
        );
        let plain = RemoteUrl::parse("  https://example.invalid/org/repo  ").unwrap();
        assert_eq!(plain.as_str(), "https://example.invalid/org/repo");
        assert_eq!(plain.to_string(), "https://example.invalid/org/repo");
        assert_eq!(
            String::from(plain),
            "https://example.invalid/org/repo".to_string()
        );
    }

    #[test]
    fn a_remote_url_must_be_https() {
        for raw in [
            "http://example.invalid/org/repo.git",
            "git@example.invalid:org/repo.git",
            "ssh://example.invalid/org/repo.git",
            "example.invalid/org/repo.git",
            "HTTPS://example.invalid/org/repo.git",
            "",
        ] {
            assert_eq!(
                RemoteUrl::parse(raw),
                Err(ProjectError::InvalidRemoteUrl),
                "accepted {raw:?}"
            );
        }
    }

    /// The test-only `file://` exception, and the release behaviour it must
    /// not change; see [`RemoteUrl::parse`].
    #[test]
    #[cfg(feature = "integration-tests")]
    fn a_local_remote_is_accepted_only_under_the_test_feature() {
        assert_eq!(
            RemoteUrl::parse("file:///srv/repo.git").unwrap().as_str(),
            "file:///srv/repo.git"
        );
        // Still a URL, and still absolute: a relative or host-bearing one is
        // not what the test harness produces.
        for raw in ["file://srv/repo.git", "file://", "file:/srv/repo.git"] {
            assert_eq!(
                RemoteUrl::parse(raw),
                Err(ProjectError::InvalidRemoteUrl),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    #[cfg(not(feature = "integration-tests"))]
    fn a_local_remote_is_rejected_by_a_release_build() {
        assert_eq!(
            RemoteUrl::parse("file:///srv/repo.git"),
            Err(ProjectError::InvalidRemoteUrl)
        );
    }

    #[test]
    fn a_remote_url_rejects_embedded_credentials() {
        for raw in [
            "https://user@example.invalid/org/repo.git",
            "https://user:not-a-real-token@example.invalid/org/repo.git",
            "https://@example.invalid/org/repo.git",
        ] {
            assert_eq!(
                RemoteUrl::parse(raw),
                Err(ProjectError::RemoteUrlHasCredentials),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn a_remote_url_rejects_a_fragment() {
        assert_eq!(
            RemoteUrl::parse("https://example.invalid/org/repo.git#main"),
            Err(ProjectError::InvalidRemoteUrl)
        );
    }

    #[test]
    fn a_remote_url_keeps_a_query_string() {
        // Not userinfo, and some self-hosted forges really do serve one.
        assert!(RemoteUrl::parse("https://example.invalid/r?p=org/repo.git").is_ok());
    }

    #[test]
    fn a_remote_url_allows_an_at_sign_in_the_path() {
        // Only the authority is userinfo; a `@` further along is a path
        // character some forges really do produce.
        assert!(RemoteUrl::parse("https://example.invalid/org/re@po.git").is_ok());
    }

    #[test]
    fn a_remote_url_rejects_whitespace_and_an_empty_host() {
        assert_eq!(
            RemoteUrl::parse("https://example.invalid/org/my repo.git"),
            Err(ProjectError::InvalidRemoteUrl)
        );
        assert_eq!(
            RemoteUrl::parse("https:// example.invalid/repo.git"),
            Err(ProjectError::InvalidRemoteUrl)
        );
        assert_eq!(
            RemoteUrl::parse("https:///org/repo.git"),
            Err(ProjectError::InvalidRemoteUrl)
        );
        assert_eq!(
            RemoteUrl::parse("https://"),
            Err(ProjectError::InvalidRemoteUrl)
        );
    }

    #[test]
    fn a_remote_url_rejection_never_echoes_the_url() {
        // The rejected value is exactly what must not reach a response body or
        // a log line, because the reason it was rejected is that it carries a
        // credential (`CLAUDE.md`, rule 3).
        let message = RemoteUrl::parse("https://user:not-a-real-token@example.invalid/repo.git")
            .unwrap_err()
            .to_string();
        assert!(!message.contains("not-a-real-token"), "leaked: {message}");
        assert!(!message.contains("example.invalid"), "leaked: {message}");
    }

    #[test]
    fn a_branch_name_accepts_the_shapes_git_uses() {
        for raw in [
            "main",
            "release/1.2",
            "release/v1.0",
            "feat_x-1",
            "user/feat/x",
            "v1.0.0",
            &"b".repeat(MAX_BRANCH_NAME_CHARS),
        ] {
            assert_eq!(
                BranchName::parse(raw).expect("a branch name").as_str(),
                raw,
                "rejected {raw:?}"
            );
            assert!(is_branch_name(raw), "rejected {raw:?}");
        }

        assert_eq!(
            BranchName::parse("release/v1.0").unwrap().to_string(),
            "release/v1.0"
        );
        assert_eq!(
            String::from(BranchName::parse("main").unwrap()),
            "main".to_string()
        );
    }

    #[test]
    fn a_branch_name_rejects_what_git_would_not_store_or_would_misread() {
        for raw in [
            // Empty, too long, whitespace, control characters.
            "",
            " ",
            "my branch",
            "main\n",
            "\tmain",
            "ma\u{7}in",
            &"b".repeat(MAX_BRANCH_NAME_CHARS + 1),
            // Revision-expression and glob characters.
            "a..b",
            "main~1",
            "main^",
            "origin:main",
            "what?",
            "feat/*",
            "feat[1]",
            "feat\\x",
            "main@{upstream}",
            // Leading and trailing forms.
            "-x",
            "/main",
            "refs/heads/x",
            "a/",
            "a.",
            "a.lock",
        ] {
            assert_eq!(
                BranchName::parse(raw),
                Err(ProjectError::InvalidDefaultBranch),
                "accepted {raw:?}"
            );
            assert!(!is_branch_name(raw), "accepted {raw:?}");
        }
    }

    #[test]
    fn max_attempts_accepts_one_through_twenty() {
        for raw in MIN_MAX_ATTEMPTS..=MAX_MAX_ATTEMPTS {
            assert_eq!(MaxAttempts::parse(raw).unwrap().get(), raw);
        }
        assert_eq!(MaxAttempts::default().get(), DEFAULT_MAX_ATTEMPTS);
        assert_eq!(MaxAttempts::default().to_string(), "3");
    }

    #[test]
    fn max_attempts_rejects_zero_negatives_and_twenty_one() {
        for raw in [i16::MIN, -1, 0, MAX_MAX_ATTEMPTS + 1, i16::MAX] {
            assert_eq!(
                MaxAttempts::parse(raw),
                Err(ProjectError::InvalidMaxAttempts),
                "accepted {raw}"
            );
        }
    }

    #[test]
    fn a_new_project_starts_with_the_documented_defaults() {
        let project = NewProject::new("  Mars  ", "https://example.invalid/org/repo.git").unwrap();
        assert_eq!(project.name.as_str(), "Mars");
        assert_eq!(
            project.remote_url.as_str(),
            "https://example.invalid/org/repo.git"
        );
        assert!(project.default_branch.is_none());
        assert!(project.created_by.is_none());
        assert_eq!(project.max_attempts.get(), DEFAULT_MAX_ATTEMPTS);
    }

    #[test]
    fn a_new_project_rejects_an_invalid_name_or_remote() {
        assert_eq!(
            NewProject::new("", "https://example.invalid/org/repo.git").unwrap_err(),
            ProjectError::InvalidName
        );
        assert_eq!(
            NewProject::new("mars", "http://example.invalid/org/repo.git").unwrap_err(),
            ProjectError::InvalidRemoteUrl
        );
    }

    #[test]
    fn an_empty_update_changes_nothing() {
        assert!(ProjectUpdate::default().is_empty());
        assert!(
            !ProjectUpdate {
                max_attempts: Some(MaxAttempts::parse(5).unwrap()),
                ..ProjectUpdate::default()
            }
            .is_empty()
        );
    }
}
