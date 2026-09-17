//! Ref naming, resolution and the ref plumbing every later git operation
//! builds on.
//!
//! `ARCHITECTURE.md`, "Git model" (Ref ownership) and ADR 0017: the project
//! repository is not an exact mirror. It owns five namespaces and nothing
//! else:
//!
//! | Namespace | Kind | Mutation target | Push source |
//! | --- | --- | --- | --- |
//! | `refs/heads/<b>` | integration head | yes | yes |
//! | `refs/remotes/origin/<b>` | upstream tracking | no | no |
//! | `refs/tags/<t>` | upstream tag | no | no |
//! | `refs/sessions/<sid>` | session work | yes | yes |
//! | `refs/handoffs/<id>` | retained hand-off commit (ADR 0018) | no | no |
//!
//! [`GitRef`] is that table as a type. Everything a user, an agent or a cron
//! job can name goes through [`GitRef::parse`] once, and the predicates —
//! [`GitRef::is_mutation_target`], [`GitRef::is_push_source`],
//! [`GitRef::is_merge_source`], [`GitRef::is_base`] — are how each endpoint
//! enforces the column it cares about, so "upstream refs cannot be mutation
//! targets or push sources (400)" (`SPEC.md`, "Git") has one implementation
//! rather than one per route.
//!
//! The rest of the module is the plumbing the later tasks share:
//! [`resolve`] (`rev-parse --verify`), [`list`] (`for-each-ref`), [`update`]
//! and [`delete`] (`update-ref`), and the hand-off retention primitives
//! [`retain_handoff`], [`remove_handoff`] and [`list_handoffs`] that code
//! hand-offs and the orphan-cleanup job call (`docs/data-model.md`,
//! `task_handoffs`). Every one of them runs through [`GitCommand`]; nothing
//! here spawns a process of its own (ADR 0011).
//!
//! Callers hold the project git lock. Nothing in this module takes it: these
//! are the helpers composite operations call after acquiring it once
//! (`ARCHITECTURE.md`, "Git model", Serialization).

use std::path::Path;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{GitCommand, GitError};
use crate::models::{Branch, BranchKind};
use crate::prelude::*;

/// The integration-head namespace: Mars's own branches (ADR 0017).
const HEADS: &str = "refs/heads/";
/// The upstream-tracking namespace. Read-only; a fetch refreshes it and
/// nothing else writes it.
const UPSTREAM: &str = "refs/remotes/origin/";
/// The tag namespace, as fetched from upstream.
const TAGS: &str = "refs/tags/";
/// The session-work namespace, written by fetch-back and rebase write-back.
const SESSIONS: &str = "refs/sessions/";
/// The hand-off namespace: one immutable commit per hand-off (ADR 0018).
const HANDOFFS: &str = "refs/handoffs/";

/// The upstream API prefix: `origin/main` is `refs/remotes/origin/main`.
const ORIGIN_PREFIX: &str = "origin/";

/// The symbolic ref a clone leaves behind to record upstream's default branch.
/// Not a branch, so [`list`] drops it (`SPEC.md`, "Projects").
const ORIGIN_HEAD: &str = "refs/remotes/origin/HEAD";

/// The length of a full SHA-1 object id.
///
/// SHA-1 only in v1: the project repository is created by `git init --bare`
/// with the default object format, so every id git hands back here is 40
/// lowercase hex characters.
const OID_LEN: usize = 40;

/// The canonical hyphenated UUID length. A session or hand-off id is only ever
/// spelled this way, in the API and in the ref name.
const UUID_LEN: usize = 36;

/// The `for-each-ref` format: NUL between fields, newline between refs.
///
/// The fourth field is the deliberate addition to the three
/// `ARCHITECTURE.md`-shaped ones. `%(committerdate)` is empty for an annotated
/// tag, whose ref points at a tag object rather than a commit;
/// `%(*committerdate)` is the same field on the commit that tag peels to. Only
/// the first non-empty of the two is used, so a tag entry still carries a
/// usable date and nothing is silently dropped. [`to_branch`] never returns a
/// tag either way.
const REF_FORMAT: &str = "--format=%(refname)%00%(objectname)%00%(committerdate:iso-strict)%00%(*committerdate:iso-strict)";

/// Anything the API, an agent or a stored row can name as a ref.
///
/// Construct one with [`GitRef::parse`]; the variants carry the *short* name,
/// never the fully qualified one, so `Head("main")` and
/// `Upstream("main")` are distinct values rather than two spellings that have
/// to be compared as strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GitRef {
    /// A Mars integration head, `refs/heads/<b>`, named `main`.
    Head(String),
    /// An upstream-tracking ref, `refs/remotes/origin/<b>`, named
    /// `origin/main`. Read-only (ADR 0017).
    Upstream(String),
    /// An upstream tag, `refs/tags/<t>`. Reachable only by fully qualified
    /// name or through [`resolve`]'s fallback; see [`GitRef::parse`].
    Tag(String),
    /// A session's synced work, `refs/sessions/<sid>`.
    Session(Uuid),
    /// A hand-off's retained commit, `refs/handoffs/<id>` (ADR 0018). Never a
    /// mutation target or a push source; selected by hand-off id.
    Handoff(Uuid),
    /// A full object id naming a commit directly. The one variant with no ref
    /// behind it.
    Commit(String),
}

impl GitRef {
    /// Turn an API name into a ref.
    ///
    /// The precedence, in order, and it is a precedence rather than a
    /// disjunction because the forms overlap:
    ///
    /// 1. A fully qualified name in one of the five owned namespaces —
    ///    `refs/heads/`, `refs/remotes/origin/`, `refs/tags/`,
    ///    `refs/sessions/`, `refs/handoffs/`. Any other `refs/…` is rejected:
    ///    the repository owns no other namespace.
    /// 2. A UUID: a session ref. **A session id always wins over a branch
    ///    literally named like a UUID.**
    ///    Spell `refs/heads/<uuid>` to mean that branch.
    /// 3. Forty lowercase hex characters: a commit. **A commit id wins even if
    ///    a branch has that name.** Spell `refs/heads/<sha>` to mean the
    ///    branch.
    /// 4. `origin/<name>`: an upstream-tracking ref.
    /// 5. Anything else: an integration head.
    ///
    /// A bare tag name is therefore *not* a tag here — it parses as a head.
    /// That is deliberate: [`resolve`] retries a head that does not exist as a
    /// tag, so `v1.0.0` still works, and a head that shares a tag's name wins
    /// (`ARCHITECTURE.md`, "Git model", Ref ownership: integration heads are
    /// what Mars owns). Fully qualified names disambiguate every case.
    ///
    /// Short names must pass git's own `check-ref-format --branch` rules, so
    /// nothing that reaches argv can be mistaken for an option or a revision
    /// expression.
    pub fn parse(input: &str) -> std::result::Result<Self, GitError> {
        if let Some(rest) = input.strip_prefix("refs/") {
            return Self::parse_qualified(input, rest);
        }

        if let Some(id) = parse_uuid(input) {
            return Ok(GitRef::Session(id));
        }

        if is_object_id(input) {
            return Ok(GitRef::Commit(input.to_string()));
        }

        if let Some(name) = input.strip_prefix(ORIGIN_PREFIX) {
            validate_name(name)?;
            return Ok(GitRef::Upstream(name.to_string()));
        }

        validate_name(input)?;
        Ok(GitRef::Head(input.to_string()))
    }

    /// The `refs/…` half of [`GitRef::parse`], split out to keep the
    /// precedence above readable. `rest` is `input` without the `refs/`.
    fn parse_qualified(input: &str, rest: &str) -> std::result::Result<Self, GitError> {
        if let Some(name) = rest.strip_prefix("heads/") {
            validate_name(name)?;
            return Ok(GitRef::Head(name.to_string()));
        }
        if let Some(name) = rest.strip_prefix("remotes/origin/") {
            validate_name(name)?;
            return Ok(GitRef::Upstream(name.to_string()));
        }
        if let Some(name) = rest.strip_prefix("tags/") {
            validate_name(name)?;
            return Ok(GitRef::Tag(name.to_string()));
        }
        if let Some(id) = rest.strip_prefix("sessions/") {
            return parse_uuid(id)
                .map(GitRef::Session)
                .ok_or_else(|| GitError::InvalidRef(input.to_string()));
        }
        if let Some(id) = rest.strip_prefix("handoffs/") {
            return parse_uuid(id)
                .map(GitRef::Handoff)
                .ok_or_else(|| GitError::InvalidRef(input.to_string()));
        }

        Err(GitError::InvalidRef(input.to_string()))
    }

    /// The fully qualified ref name, or `None` for a [`GitRef::Commit`], which
    /// is an object id and not a ref at all.
    pub fn full_name(&self) -> Option<String> {
        match self {
            GitRef::Head(name) => Some(format!("{HEADS}{name}")),
            GitRef::Upstream(name) => Some(format!("{UPSTREAM}{name}")),
            GitRef::Tag(name) => Some(format!("{TAGS}{name}")),
            GitRef::Session(id) => Some(session_ref(*id)),
            GitRef::Handoff(id) => Some(handoff_ref(*id)),
            GitRef::Commit(_) => None,
        }
    }

    /// The API spelling: what a response carries and what [`GitRef::parse`]
    /// accepts back. A tag and a head with the same name share a spelling,
    /// which is what the parse precedence above is about.
    pub fn api_name(&self) -> String {
        match self {
            GitRef::Head(name) | GitRef::Tag(name) | GitRef::Commit(name) => name.clone(),
            GitRef::Upstream(name) => format!("{ORIGIN_PREFIX}{name}"),
            GitRef::Session(id) | GitRef::Handoff(id) => id.to_string(),
        }
    }

    /// May an operation write this ref? Integration heads and session refs
    /// only (`ARCHITECTURE.md`, "Git model", Ref ownership).
    pub fn is_mutation_target(&self) -> bool {
        matches!(self, GitRef::Head(_) | GitRef::Session(_))
    }

    /// May a push send this ref upstream? The same two: a push sends exactly
    /// one selected integration head or session ref.
    pub fn is_push_source(&self) -> bool {
        matches!(self, GitRef::Head(_) | GitRef::Session(_))
    }

    /// May a merge take its source from here? Everything except a tag: a
    /// hand-off's retained commit is merged by a task merge (ADR 0018), and a
    /// commit id and an upstream ref are both explicit merge sources.
    pub fn is_merge_source(&self) -> bool {
        matches!(
            self,
            GitRef::Head(_)
                | GitRef::Upstream(_)
                | GitRef::Session(_)
                | GitRef::Handoff(_)
                | GitRef::Commit(_)
        )
    }

    /// May this be a rebase `onto` or a diff `base`? Integration and
    /// upstream-tracking branch names only (`SPEC.md`, "Git").
    pub fn is_base(&self) -> bool {
        matches!(self, GitRef::Head(_) | GitRef::Upstream(_))
    }

    /// What `rev-parse` is given: the fully qualified name, or the object id
    /// itself for a [`GitRef::Commit`].
    fn rev_spec(&self) -> String {
        match self.full_name() {
            Some(full) => full,
            None => self.api_name(),
        }
    }
}

/// The ref name a session's work lives at (`ARCHITECTURE.md`, "Git model").
pub fn session_ref(session_id: Uuid) -> String {
    format!("{SESSIONS}{session_id}")
}

/// The ref name a hand-off's commit is retained at (`docs/data-model.md`,
/// `task_handoffs`).
pub fn handoff_ref(handoff_id: Uuid) -> String {
    format!("{HANDOFFS}{handoff_id}")
}

/// Is `raw` a full lowercase object id?
///
/// Abbreviated and uppercase ids are rejected on purpose: every id this module
/// returns is what git printed, and every id it accepts has to compare equal
/// to one of those.
fn is_object_id(raw: &str) -> bool {
    raw.len() == OID_LEN && raw.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// A UUID in the one spelling Mars uses: canonical, hyphenated, 36
/// characters. `Uuid::parse_str` also accepts braced and unhyphenated forms,
/// and accepting those here would quietly turn a 32-character branch name into
/// a session ref.
fn parse_uuid(raw: &str) -> Option<Uuid> {
    if raw.len() != UUID_LEN {
        return None;
    }
    Uuid::parse_str(raw).ok()
}

/// git's `check-ref-format --branch` rules, applied to a short name.
///
/// Reimplemented rather than shelled out to: it is called on every request, a
/// process per name would be absurd, and the rules are fixed. The extra rule
/// at the end is Mars's: a short name may not start with `refs/`, because a
/// re-nested name would resolve to a ref in a namespace the caller did not
/// select.
fn validate_name(name: &str) -> std::result::Result<(), GitError> {
    let invalid = || GitError::InvalidRef(name.to_string());

    if name.is_empty() || name == "@" {
        return Err(invalid());
    }
    // A leading `-` would be an option to whichever command received it. The
    // commands here also pass `--end-of-options`; this is the first line.
    if name.starts_with('-') {
        return Err(invalid());
    }
    if name.starts_with("refs/") {
        return Err(invalid());
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") {
        return Err(invalid());
    }
    if name.starts_with('/') || name.ends_with('/') || name.ends_with('.') {
        return Err(invalid());
    }
    // The characters git reserves for revision syntax, plus whitespace and
    // control characters.
    if name.chars().any(|c| {
        c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return Err(invalid());
    }
    // Per path component: no leading dot, no `.lock` suffix.
    if name
        .split('/')
        .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(invalid());
    }

    Ok(())
}

/// A ref together with the commit it resolves to, at the moment it was read.
///
/// `git_ref` is the kind that actually resolved, which is not always the kind
/// that was asked for: [`resolve`] hands back a [`GitRef::Tag`] when a bare
/// name turned out to be one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRef {
    /// The ref that resolved.
    pub git_ref: GitRef,
    /// Its commit, as a full object id.
    pub commit: String,
}

/// Resolve `git_ref` against the project repository at `mirror`.
///
/// `git rev-parse --verify --end-of-options <name>^{commit}`: `--verify`
/// makes git refuse anything ambiguous or absent instead of echoing it back,
/// `^{commit}` peels an annotated tag to its commit, and `--end-of-options`
/// means a name could not become an option even if [`GitRef::parse`] had let
/// one through.
///
/// A [`GitRef::Head`] that does not exist is retried as a tag before giving
/// up, which is what makes a bare tag name usable at all; a head therefore
/// wins over a tag of the same name. The failures are distinguished:
/// [`GitError::NotACommit`] when the name resolves to a non-commit object,
/// [`GitError::UnknownRef`] when nothing resolves.
pub async fn resolve(
    mirror: &Path,
    git_ref: &GitRef,
) -> std::result::Result<ResolvedRef, GitError> {
    let spec = git_ref.rev_spec();

    if let Some(commit) = peel(mirror, &spec, "commit").await? {
        return Ok(ResolvedRef {
            git_ref: git_ref.clone(),
            commit,
        });
    }

    // The documented fallback: a bare name parses as a head, so this is the
    // only way `v1.0.0` reaches `refs/tags/v1.0.0`.
    if let GitRef::Head(name) = git_ref {
        let tag = GitRef::Tag(name.clone());
        if let Some(commit) = peel(mirror, &tag.rev_spec(), "commit").await? {
            return Ok(ResolvedRef {
                git_ref: tag,
                commit,
            });
        }
    }

    // `^{object}` succeeds for any object that exists and peels to no
    // particular type, so it separates "points at a tree" from "is not there".
    if peel(mirror, &spec, "object").await?.is_some() {
        return Err(GitError::NotACommit(git_ref.api_name()));
    }

    // Before calling it a missing ref, make sure the repository itself is
    // there: without this, a wrong mirror path would answer 400 "no such ref"
    // for every name instead of the 500 it is.
    GitCommand::new()
        .args(["rev-parse", "--git-dir"])
        .cwd(mirror)
        .run_ok()
        .await?;

    Err(GitError::UnknownRef(git_ref.api_name()))
}

/// One `rev-parse --verify <spec>^{<peel>}`, as `Some(oid)` or `None` when git
/// refused.
///
/// A non-zero exit is an answer here rather than a failure — the caller turns
/// it into the right [`GitError`] — so this uses [`GitCommand::run`].
async fn peel(
    mirror: &Path,
    spec: &str,
    peel: &str,
) -> std::result::Result<Option<String>, GitError> {
    let arg = format!("{spec}^{{{peel}}}");
    let output = GitCommand::new()
        .args(["rev-parse", "--verify", "--end-of-options", &arg])
        .cwd(mirror)
        .run()
        .await?;

    if output.status != 0 {
        return Ok(None);
    }

    let oid = output.stdout.trim();
    if !is_object_id(oid) {
        // git printed something that is not an object id after exiting 0,
        // which is not a state a caller can act on.
        return Err(GitError::Command {
            args: vec!["rev-parse".to_string(), "--verify".to_string(), arg],
            code: Some(output.status),
            stderr: "rev-parse exited 0 without printing a full object id".to_string(),
        });
    }

    Ok(Some(oid.to_string()))
}

/// One ref as `for-each-ref` reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEntry {
    /// The fully qualified ref name.
    pub full_name: String,
    /// The object the ref points at — the *tag object* for an annotated tag,
    /// not the commit it peels to. [`Branch`] excludes tags for exactly this
    /// reason; [`resolve`] is what peels.
    pub commit: String,
    /// The committer date of that commit, or of the commit an annotated tag
    /// peels to.
    pub committer_date: DateTime<Utc>,
}

/// List the refs matching `patterns`, newest field format as documented on
/// [`REF_FORMAT`].
///
/// `refs/remotes/origin/HEAD` is dropped: it is the symbolic record of
/// upstream's default branch, not a branch of its own. A line that does not
/// parse is skipped with a `warn!` rather than failing the listing, so one
/// malformed ref cannot make a project's branch list unavailable.
pub async fn list(
    mirror: &Path,
    patterns: &[&str],
) -> std::result::Result<Vec<RefEntry>, GitError> {
    let output = GitCommand::new()
        .args(["for-each-ref", REF_FORMAT, "--end-of-options"])
        .args(patterns)
        .cwd(mirror)
        .run_ok()
        .await?;

    let mut entries = Vec::new();
    for line in output.stdout.lines() {
        if line.is_empty() {
            continue;
        }

        let mut fields = line.split('\0');
        let (Some(full_name), Some(commit), Some(date), peeled_date, None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            warn!("skipping a for-each-ref line with an unexpected field count");
            continue;
        };

        if full_name == ORIGIN_HEAD {
            continue;
        }

        if !is_object_id(commit) {
            warn!(git.refname = %full_name, "skipping a ref without a full object id");
            continue;
        }

        let raw_date = if date.is_empty() {
            peeled_date.unwrap_or_default()
        } else {
            date
        };
        let Ok(committer_date) = DateTime::parse_from_rfc3339(raw_date) else {
            warn!(git.refname = %full_name, "skipping a ref with an unreadable committer date");
            continue;
        };

        entries.push(RefEntry {
            full_name: full_name.to_string(),
            commit: commit.to_string(),
            committer_date: committer_date.with_timezone(&Utc),
        });
    }

    Ok(entries)
}

/// The API view of a ref, or `None` when it is not one of the three kinds the
/// API calls a branch (`SPEC.md`, "Projects").
///
/// Tags and hand-off refs are the `None` cases: a tag is not a branch, and
/// hand-off refs are internal and exposed by hand-off id instead.
pub fn to_branch(entry: &RefEntry) -> Option<Branch> {
    let git_ref = GitRef::parse(&entry.full_name).ok()?;

    let kind = match git_ref {
        GitRef::Head(_) => BranchKind::Head,
        // `origin/HEAD` is filtered in `list`; this is the same rule applied
        // to an entry from anywhere else.
        GitRef::Upstream(ref name) if name == "HEAD" => return None,
        GitRef::Upstream(_) => BranchKind::Upstream,
        GitRef::Session(_) => BranchKind::Session,
        GitRef::Tag(_) | GitRef::Handoff(_) | GitRef::Commit(_) => return None,
    };

    Some(Branch {
        name: git_ref.api_name(),
        kind,
        commit: entry.commit.clone(),
        session_id: match git_ref {
            GitRef::Session(id) => Some(id),
            _ => None,
        },
    })
}

/// Point `full_name` at `new_commit`.
///
/// `expected_old` is git's own compare-and-swap: `update-ref` refuses unless
/// the ref is already at that value, and the all-zero id means "must not exist
/// yet". A refusal comes back as [`GitError::Command`] carrying git's exit
/// code, because a caller holding the project git lock that finds a ref has
/// moved underneath it has an invariant failure, not a user-visible conflict
/// (`ARCHITECTURE.md`, "Git model", Serialization).
///
/// `full_name` must be a fully qualified name in one of the five owned
/// namespaces; anything else is [`GitError::InvalidRef`], so no operation can
/// write a ref the repository does not own.
pub async fn update(
    mirror: &Path,
    full_name: &str,
    new_commit: &str,
    expected_old: Option<&str>,
) -> std::result::Result<(), GitError> {
    owned_ref(full_name)?;

    if !is_object_id(new_commit) {
        return Err(GitError::InvalidRef(new_commit.to_string()));
    }
    if let Some(old) = expected_old
        && !is_object_id(old)
    {
        return Err(GitError::InvalidRef(old.to_string()));
    }

    let mut command =
        GitCommand::new().args(["update-ref", "--end-of-options", full_name, new_commit]);
    if let Some(old) = expected_old {
        command = command.arg(old);
    }
    command.cwd(mirror).run_ok().await?;

    Ok(())
}

/// Delete `full_name`.
///
/// Idempotent, because `git update-ref -d` is: deleting a ref that is not
/// there succeeds. The same namespace check as [`update`] applies.
pub async fn delete(mirror: &Path, full_name: &str) -> std::result::Result<(), GitError> {
    owned_ref(full_name)?;

    GitCommand::new()
        .args(["update-ref", "-d", "--end-of-options", full_name])
        .cwd(mirror)
        .run_ok()
        .await?;

    Ok(())
}

/// Accept `full_name` only when it is the fully qualified spelling of a ref in
/// one of the five owned namespaces.
///
/// [`GitRef::parse`] would happily read `main` as a head; requiring the parsed
/// value to spell itself back identically is what makes a short name — and a
/// commit id, which has no ref at all — unusable as a write target.
fn owned_ref(full_name: &str) -> std::result::Result<GitRef, GitError> {
    let git_ref = GitRef::parse(full_name)?;

    if git_ref.full_name().as_deref() == Some(full_name) {
        Ok(git_ref)
    } else {
        Err(GitError::InvalidRef(full_name.to_string()))
    }
}

/// Pin `commit` at `refs/handoffs/<handoff_id>` so it survives the source
/// session, its branch moving on and the branch's deletion (ADR 0018).
///
/// The commit is verified to be present *and* to be a commit before the ref is
/// written: a hand-off names code to review, and a ref pointing at a tree or
/// at nothing would be discovered at the next launch instead of here.
/// Callers hold the project git lock (`docs/data-model.md`, `task_handoffs`).
pub async fn retain_handoff(
    mirror: &Path,
    handoff_id: Uuid,
    commit: &str,
) -> std::result::Result<(), GitError> {
    if !is_object_id(commit) {
        return Err(GitError::InvalidRef(commit.to_string()));
    }

    let resolved = resolve(mirror, &GitRef::Commit(commit.to_string())).await?;
    if resolved.commit != commit {
        // `^{commit}` peeled to something else, so the id named a tag object
        // rather than the commit the caller believes it is pinning.
        return Err(GitError::NotACommit(commit.to_string()));
    }

    update(mirror, &handoff_ref(handoff_id), commit, None).await
}

/// Remove a hand-off's retained ref.
///
/// Idempotent: task and project deletion call it under the project git lock,
/// and the orphan-cleanup job calls it again for refs an interrupted deletion
/// left behind, so a missing ref is a success.
pub async fn remove_handoff(mirror: &Path, handoff_id: Uuid) -> std::result::Result<(), GitError> {
    delete(mirror, &handoff_ref(handoff_id)).await
}

/// Every retained hand-off ref, as `(hand-off id, commit)`.
///
/// What the orphan-cleanup job compares against `task_handoffs` to find refs
/// left by an interrupted deletion or a failed publication
/// (`docs/data-model.md`, `task_handoffs`).
pub async fn list_handoffs(mirror: &Path) -> std::result::Result<Vec<(Uuid, String)>, GitError> {
    let entries = list(mirror, &["refs/handoffs/*"]).await?;

    let mut handoffs = Vec::with_capacity(entries.len());
    for entry in entries {
        match GitRef::parse(&entry.full_name) {
            Ok(GitRef::Handoff(id)) => handoffs.push((id, entry.commit)),
            _ => warn!(
                git.refname = %entry.full_name,
                "skipping a hand-off ref whose name is not a hand-off id"
            ),
        }
    }

    Ok(handoffs)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::git::testutil::{TestUpstream, run_git};

    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn uuid() -> Uuid {
        Uuid::new_v4()
    }

    // --- parse ------------------------------------------------------------

    #[test]
    fn a_bare_name_is_an_integration_head() {
        assert_eq!(
            GitRef::parse("main").unwrap(),
            GitRef::Head("main".to_string())
        );
        assert_eq!(
            GitRef::parse("release/v1.0").unwrap(),
            GitRef::Head("release/v1.0".to_string())
        );
    }

    #[test]
    fn an_origin_prefix_is_upstream_tracking() {
        assert_eq!(
            GitRef::parse("origin/main").unwrap(),
            GitRef::Upstream("main".to_string())
        );
        // `origin` on its own is an ordinary branch name.
        assert_eq!(
            GitRef::parse("origin").unwrap(),
            GitRef::Head("origin".to_string())
        );
    }

    #[test]
    fn every_fully_qualified_namespace_parses_to_its_kind() {
        let session = uuid();
        let handoff = uuid();

        assert_eq!(
            GitRef::parse("refs/heads/main").unwrap(),
            GitRef::Head("main".to_string())
        );
        assert_eq!(
            GitRef::parse("refs/remotes/origin/main").unwrap(),
            GitRef::Upstream("main".to_string())
        );
        assert_eq!(
            GitRef::parse("refs/tags/v1.0.0").unwrap(),
            GitRef::Tag("v1.0.0".to_string())
        );
        assert_eq!(
            GitRef::parse(&format!("refs/sessions/{session}")).unwrap(),
            GitRef::Session(session)
        );
        assert_eq!(
            GitRef::parse(&format!("refs/handoffs/{handoff}")).unwrap(),
            GitRef::Handoff(handoff)
        );
    }

    #[test]
    fn a_uuid_is_a_session_ref_even_though_it_could_be_a_branch_name() {
        let id = uuid();

        assert_eq!(GitRef::parse(&id.to_string()).unwrap(), GitRef::Session(id));
        // The documented escape hatch.
        assert_eq!(
            GitRef::parse(&format!("refs/heads/{id}")).unwrap(),
            GitRef::Head(id.to_string())
        );
    }

    #[test]
    fn forty_hex_characters_are_a_commit_even_though_they_could_be_a_branch() {
        let sha = "a".repeat(40);

        assert_eq!(GitRef::parse(&sha).unwrap(), GitRef::Commit(sha.clone()));
        assert_eq!(
            GitRef::parse(&format!("refs/heads/{sha}")).unwrap(),
            GitRef::Head(sha)
        );
        // Abbreviated and uppercase ids are names, not object ids.
        assert_eq!(
            GitRef::parse("abc1234").unwrap(),
            GitRef::Head("abc1234".to_string())
        );
        assert_eq!(
            GitRef::parse(&"A".repeat(40)).unwrap(),
            GitRef::Head("A".repeat(40))
        );
    }

    #[test]
    fn a_bare_tag_name_parses_as_a_head_and_only_resolve_finds_the_tag() {
        assert_eq!(
            GitRef::parse("v1.0.0").unwrap(),
            GitRef::Head("v1.0.0".to_string())
        );
    }

    #[test]
    fn names_git_would_refuse_are_rejected() {
        for bad in [
            "",
            "..",
            "a..b",
            "-x",
            "--force",
            "a.lock",
            "feature/a.lock",
            "refs/foo",
            "refs/heads/refs/nested",
            "with space",
            "ctrl\u{1}char",
            "at@{one}",
            "trailing.",
            "/leading",
            "trailing/",
            "double//slash",
            ".hidden",
            "a/.hidden/b",
            "@",
            "tilde~1",
            "caret^",
            "colon:name",
            "star*",
            "question?",
            "bracket[1]",
            "back\\slash",
            "origin/",
            "origin/-x",
        ] {
            assert!(
                GitRef::parse(bad).is_err(),
                "{bad:?} should not have parsed"
            );
        }
    }

    #[test]
    fn an_unqualified_session_or_handoff_name_is_rejected_rather_than_guessed() {
        assert!(GitRef::parse("refs/sessions/not-a-uuid").is_err());
        assert!(GitRef::parse("refs/handoffs/not-a-uuid").is_err());
        // Unhyphenated and braced UUID spellings are not the API spelling.
        let id = uuid();
        assert_eq!(
            GitRef::parse(&id.simple().to_string()).unwrap(),
            GitRef::Head(id.simple().to_string())
        );
    }

    #[test]
    fn a_rejected_name_is_an_invalid_ref_naming_itself() {
        let error = GitRef::parse("a..b").unwrap_err();
        assert!(matches!(error, GitError::InvalidRef(ref name) if name == "a..b"));
        assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    // --- naming and predicates -------------------------------------------

    #[test]
    fn full_and_api_names_are_the_documented_spellings() {
        let session = uuid();
        let handoff = uuid();
        let sha = "b".repeat(40);

        for (git_ref, full, api) in [
            (
                GitRef::Head("main".into()),
                Some("refs/heads/main".to_string()),
                "main".to_string(),
            ),
            (
                GitRef::Upstream("main".into()),
                Some("refs/remotes/origin/main".to_string()),
                "origin/main".to_string(),
            ),
            (
                GitRef::Tag("v1.0.0".into()),
                Some("refs/tags/v1.0.0".to_string()),
                "v1.0.0".to_string(),
            ),
            (
                GitRef::Session(session),
                Some(format!("refs/sessions/{session}")),
                session.to_string(),
            ),
            (
                GitRef::Handoff(handoff),
                Some(format!("refs/handoffs/{handoff}")),
                handoff.to_string(),
            ),
            (GitRef::Commit(sha.clone()), None, sha.clone()),
        ] {
            assert_eq!(git_ref.full_name(), full, "{git_ref:?}");
            assert_eq!(git_ref.api_name(), api, "{git_ref:?}");
        }
    }

    #[test]
    fn a_full_name_parses_back_to_the_same_ref() {
        for git_ref in [
            GitRef::Head("main".into()),
            GitRef::Upstream("main".into()),
            GitRef::Tag("v1.0.0".into()),
            GitRef::Session(uuid()),
            GitRef::Handoff(uuid()),
        ] {
            let full = git_ref.full_name().expect("a ref has a full name");
            assert_eq!(GitRef::parse(&full).unwrap(), git_ref);
        }
    }

    #[test]
    fn only_heads_and_session_refs_may_be_written_or_pushed() {
        let writable = [GitRef::Head("main".into()), GitRef::Session(uuid())];
        let read_only = [
            GitRef::Upstream("main".into()),
            GitRef::Tag("v1.0.0".into()),
            GitRef::Handoff(uuid()),
            GitRef::Commit("c".repeat(40)),
        ];

        for git_ref in &writable {
            assert!(git_ref.is_mutation_target(), "{git_ref:?}");
            assert!(git_ref.is_push_source(), "{git_ref:?}");
        }
        for git_ref in &read_only {
            assert!(!git_ref.is_mutation_target(), "{git_ref:?}");
            assert!(!git_ref.is_push_source(), "{git_ref:?}");
        }
    }

    #[test]
    fn merge_sources_are_everything_but_a_tag_and_bases_are_branch_names() {
        for git_ref in [
            GitRef::Head("main".into()),
            GitRef::Upstream("main".into()),
            GitRef::Session(uuid()),
            GitRef::Handoff(uuid()),
            GitRef::Commit("d".repeat(40)),
        ] {
            assert!(git_ref.is_merge_source(), "{git_ref:?}");
        }
        assert!(!GitRef::Tag("v1.0.0".into()).is_merge_source());

        assert!(GitRef::Head("main".into()).is_base());
        assert!(GitRef::Upstream("main".into()).is_base());
        for git_ref in [
            GitRef::Tag("v1.0.0".into()),
            GitRef::Session(uuid()),
            GitRef::Handoff(uuid()),
            GitRef::Commit("e".repeat(40)),
        ] {
            assert!(!git_ref.is_base(), "{git_ref:?}");
        }
    }

    // --- against a real repository ---------------------------------------

    /// A bare project repository holding the upstream's history: its
    /// integration heads and tags from the clone, its upstream-tracking refs
    /// written here. Real mirror initialization is a later task; this is the
    /// shape that task will produce.
    struct Mirror {
        _dir: TempDir,
        path: PathBuf,
    }

    async fn mirror_of(upstream: &TestUpstream) -> Mirror {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("repo.git");

        run_git(
            dir.path(),
            &[
                "clone",
                "--bare",
                "--quiet",
                upstream.path.to_str().expect("a utf-8 path"),
                "repo.git",
            ],
        )
        .await;

        for entry in list(&upstream.path, &["refs/heads/*"])
            .await
            .expect("the upstream heads list")
        {
            let name = entry
                .full_name
                .strip_prefix(HEADS)
                .expect("a head full name");
            update(&path, &format!("{UPSTREAM}{name}"), &entry.commit, None)
                .await
                .expect("the upstream-tracking ref is written");
        }

        Mirror { _dir: dir, path }
    }

    #[tokio::test]
    async fn resolve_finds_each_kind_of_ref() {
        let upstream = TestUpstream::create().await;
        let head_commit = upstream
            .commit_file("main", "third.txt", "three\n", "feat: a third commit")
            .await;
        upstream.tag("v1.0.0", &head_commit, true).await;
        upstream.tag("light", &head_commit, false).await;
        let mirror = mirror_of(&upstream).await;

        let session = uuid();
        let handoff = uuid();
        update(&mirror.path, &session_ref(session), &head_commit, None)
            .await
            .unwrap();
        retain_handoff(&mirror.path, handoff, &head_commit)
            .await
            .unwrap();

        for git_ref in [
            GitRef::Head("main".into()),
            GitRef::Upstream("main".into()),
            GitRef::Tag("v1.0.0".into()),
            GitRef::Tag("light".into()),
            GitRef::Session(session),
            GitRef::Handoff(handoff),
            GitRef::Commit(head_commit.clone()),
        ] {
            let resolved = resolve(&mirror.path, &git_ref)
                .await
                .unwrap_or_else(|err| panic!("{git_ref:?} did not resolve: {err}"));

            assert_eq!(resolved.git_ref, git_ref);
            assert_eq!(resolved.commit, head_commit, "{git_ref:?}");
            assert!(is_object_id(&resolved.commit), "{resolved:?}");
        }
    }

    #[tokio::test]
    async fn a_bare_tag_name_resolves_through_the_head_fallback() {
        let upstream = TestUpstream::create().await;
        let tip = run_git(&upstream.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();
        upstream.tag("v2.0.0", &tip, true).await;
        let mirror = mirror_of(&upstream).await;

        let resolved = resolve(&mirror.path, &GitRef::parse("v2.0.0").unwrap())
            .await
            .expect("the tag resolves through the fallback");

        assert_eq!(resolved.git_ref, GitRef::Tag("v2.0.0".into()));
        assert_eq!(resolved.commit, tip);
    }

    #[tokio::test]
    async fn a_head_wins_over_a_tag_of_the_same_name() {
        let upstream = TestUpstream::create().await;
        let tip = run_git(&upstream.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();
        // A branch and a tag that share a name, pointing at different commits.
        let branch_commit = upstream
            .commit_file("ambiguous", "a.txt", "on the branch\n", "feat: branch")
            .await;
        upstream.tag("ambiguous", &tip, false).await;
        let mirror = mirror_of(&upstream).await;

        let resolved = resolve(&mirror.path, &GitRef::parse("ambiguous").unwrap())
            .await
            .expect("the head resolves");

        assert_eq!(resolved.git_ref, GitRef::Head("ambiguous".into()));
        assert_eq!(resolved.commit, branch_commit);
        assert_ne!(resolved.commit, tip);
    }

    #[tokio::test]
    async fn a_missing_name_is_an_unknown_ref_and_a_tree_is_not_a_commit() {
        let upstream = TestUpstream::create().await;
        let mirror = mirror_of(&upstream).await;

        for git_ref in [
            GitRef::Head("gone".into()),
            GitRef::Upstream("gone".into()),
            GitRef::Session(uuid()),
            GitRef::Handoff(uuid()),
            GitRef::Commit("f".repeat(40)),
        ] {
            let error = resolve(&mirror.path, &git_ref)
                .await
                .expect_err("a missing ref does not resolve");
            assert!(
                matches!(error, GitError::UnknownRef(ref name) if *name == git_ref.api_name()),
                "{git_ref:?} gave {error:?}"
            );
        }

        let tree = run_git(&mirror.path, &["rev-parse", "refs/heads/main^{tree}"])
            .await
            .trim()
            .to_string();
        let error = resolve(&mirror.path, &GitRef::Commit(tree.clone()))
            .await
            .expect_err("a tree is not a commit");
        assert!(matches!(error, GitError::NotACommit(ref name) if *name == tree));
    }

    #[tokio::test]
    async fn resolving_against_a_path_that_is_not_a_repository_is_an_internal_fault() {
        let dir = tempfile::tempdir().expect("a temp dir");

        let error = resolve(dir.path(), &GitRef::Head("main".into()))
            .await
            .expect_err("there is no repository there");

        assert!(
            matches!(error, GitError::Command { .. }),
            "a wrong mirror path must not read as a 400: {error:?}"
        );
    }

    #[tokio::test]
    async fn list_reports_the_three_api_kinds_and_skips_the_rest() {
        let upstream = TestUpstream::create().await;
        let tip = run_git(&upstream.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();
        upstream.tag("v3.0.0", &tip, true).await;
        let mirror = mirror_of(&upstream).await;

        let session = uuid();
        let handoff = uuid();
        update(&mirror.path, &session_ref(session), &tip, None)
            .await
            .unwrap();
        retain_handoff(&mirror.path, handoff, &tip).await.unwrap();

        let entries = list(
            &mirror.path,
            &[
                "refs/heads/*",
                "refs/remotes/origin/*",
                "refs/sessions/*",
                "refs/tags/*",
                "refs/handoffs/*",
            ],
        )
        .await
        .expect("the listing succeeds");

        // Every entry, including the annotated tag, carries a usable date.
        assert!(entries.iter().all(|entry| entry.committer_date
            > DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z").unwrap()));

        let branches: Vec<Branch> = entries.iter().filter_map(to_branch).collect();

        let head = branches
            .iter()
            .find(|branch| branch.kind == BranchKind::Head)
            .expect("the integration head is listed");
        assert_eq!(head.name, "main");
        assert_eq!(head.commit, tip);
        assert_eq!(head.session_id, None);

        let upstream_branch = branches
            .iter()
            .find(|branch| branch.kind == BranchKind::Upstream)
            .expect("the upstream-tracking ref is listed");
        assert_eq!(upstream_branch.name, "origin/main");
        assert_eq!(upstream_branch.session_id, None);

        let session_branch = branches
            .iter()
            .find(|branch| branch.kind == BranchKind::Session)
            .expect("the session ref is listed");
        assert_eq!(session_branch.name, session.to_string());
        assert_eq!(session_branch.session_id, Some(session));

        // The tag and the hand-off ref were listed but are not branches.
        assert_eq!(branches.len(), 3, "{branches:?}");
        assert!(
            entries
                .iter()
                .any(|entry| entry.full_name == "refs/tags/v3.0.0")
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.full_name == handoff_ref(handoff))
        );
    }

    #[tokio::test]
    async fn list_excludes_the_symbolic_origin_head() {
        let upstream = TestUpstream::create().await;
        let mirror = mirror_of(&upstream).await;
        let tip = run_git(&mirror.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();
        update(&mirror.path, ORIGIN_HEAD, &tip, None).await.unwrap();

        let entries = list(&mirror.path, &["refs/remotes/origin/*"])
            .await
            .expect("the listing succeeds");

        assert!(
            entries.iter().all(|entry| entry.full_name != ORIGIN_HEAD),
            "{entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.full_name == "refs/remotes/origin/main")
        );
    }

    #[tokio::test]
    async fn list_of_an_empty_namespace_is_empty_rather_than_a_failure() {
        let upstream = TestUpstream::create().await;
        let mirror = mirror_of(&upstream).await;

        assert!(
            list(&mirror.path, &["refs/handoffs/*"])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn update_honours_the_expected_old_value() {
        let upstream = TestUpstream::create().await;
        let mirror = mirror_of(&upstream).await;
        let first = run_git(&mirror.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();
        let second = run_git(&mirror.path, &["rev-parse", "refs/heads/main~1"])
            .await
            .trim()
            .to_string();

        let session = uuid();
        let name = session_ref(session);

        // The all-zero id is git's "must not exist yet".
        update(&mirror.path, &name, &first, Some(ZERO))
            .await
            .expect("creating a ref that does not exist succeeds");

        // A stale expectation is an invariant failure, not a user conflict.
        let error = update(&mirror.path, &name, &second, Some(&second))
            .await
            .expect_err("a wrong old value is refused");
        assert!(matches!(error, GitError::Command { .. }), "{error:?}");
        assert_eq!(
            error.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );

        // The ref did not move.
        assert_eq!(
            resolve(&mirror.path, &GitRef::Session(session))
                .await
                .unwrap()
                .commit,
            first
        );

        update(&mirror.path, &name, &second, Some(&first))
            .await
            .expect("the right old value succeeds");
        assert_eq!(
            resolve(&mirror.path, &GitRef::Session(session))
                .await
                .unwrap()
                .commit,
            second
        );
    }

    #[tokio::test]
    async fn update_and_delete_refuse_anything_outside_the_five_namespaces() {
        let upstream = TestUpstream::create().await;
        let mirror = mirror_of(&upstream).await;
        let tip = run_git(&mirror.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();

        for name in [
            "main",
            "origin/main",
            "refs/notes/commits",
            "refs/remotes/upstream/main",
            "HEAD",
            &tip,
        ] {
            let error = update(&mirror.path, name, &tip, None)
                .await
                .expect_err("only owned namespaces are writable");
            assert!(
                matches!(error, GitError::InvalidRef(_)),
                "{name}: {error:?}"
            );

            let error = delete(&mirror.path, name)
                .await
                .expect_err("only owned namespaces are deletable");
            assert!(
                matches!(error, GitError::InvalidRef(_)),
                "{name}: {error:?}"
            );
        }

        // A commit id that is not an object id is refused before git runs.
        let error = update(&mirror.path, &session_ref(uuid()), "not-a-sha", None)
            .await
            .expect_err("a new value must be a full object id");
        assert!(matches!(error, GitError::InvalidRef(_)), "{error:?}");
    }

    #[tokio::test]
    async fn handoff_refs_are_retained_listed_and_removed_idempotently() {
        let upstream = TestUpstream::create().await;
        let mirror = mirror_of(&upstream).await;
        let tip = run_git(&mirror.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();
        let older = run_git(&mirror.path, &["rev-parse", "refs/heads/main~1"])
            .await
            .trim()
            .to_string();

        let first = uuid();
        let second = uuid();
        retain_handoff(&mirror.path, first, &tip).await.unwrap();
        retain_handoff(&mirror.path, second, &older).await.unwrap();

        let mut listed = list_handoffs(&mirror.path).await.unwrap();
        listed.sort_by_key(|(id, _)| *id);
        let mut expected = vec![(first, tip.clone()), (second, older.clone())];
        expected.sort_by_key(|(id, _)| *id);
        assert_eq!(listed, expected);

        // The pinned commit survives the branch moving on.
        upstream
            .commit_file("main", "later.txt", "later\n", "feat: later")
            .await;
        assert_eq!(
            resolve(&mirror.path, &GitRef::Handoff(first))
                .await
                .unwrap()
                .commit,
            tip
        );

        remove_handoff(&mirror.path, first).await.unwrap();
        // Idempotent: the cleanup job runs after the deletion path.
        remove_handoff(&mirror.path, first).await.unwrap();
        remove_handoff(&mirror.path, uuid()).await.unwrap();

        assert_eq!(
            list_handoffs(&mirror.path).await.unwrap(),
            vec![(second, older)]
        );
    }

    #[tokio::test]
    async fn retain_handoff_refuses_anything_that_is_not_a_commit_in_this_repository() {
        let upstream = TestUpstream::create().await;
        let mirror = mirror_of(&upstream).await;
        let handoff = uuid();

        // Not an object id at all.
        assert!(matches!(
            retain_handoff(&mirror.path, handoff, "main")
                .await
                .unwrap_err(),
            GitError::InvalidRef(_)
        ));

        // A well-formed id that is not in this repository.
        assert!(matches!(
            retain_handoff(&mirror.path, handoff, &"a".repeat(40))
                .await
                .unwrap_err(),
            GitError::UnknownRef(_)
        ));

        // An object that is not a commit.
        let tree = run_git(&mirror.path, &["rev-parse", "refs/heads/main^{tree}"])
            .await
            .trim()
            .to_string();
        assert!(matches!(
            retain_handoff(&mirror.path, handoff, &tree)
                .await
                .unwrap_err(),
            GitError::NotACommit(_)
        ));

        // An annotated tag's own object id is not the commit it names.
        let tip = run_git(&mirror.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim()
            .to_string();
        upstream.tag("v9.0.0", &tip, true).await;
        run_git(
            &mirror.path,
            &["fetch", "--quiet", "origin", "+refs/tags/*:refs/tags/*"],
        )
        .await;
        let tag_object = run_git(&mirror.path, &["rev-parse", "refs/tags/v9.0.0"])
            .await
            .trim()
            .to_string();
        assert_ne!(tag_object, tip);
        assert!(matches!(
            retain_handoff(&mirror.path, handoff, &tag_object)
                .await
                .unwrap_err(),
            GitError::NotACommit(_)
        ));

        // None of the refusals wrote a ref.
        assert!(list_handoffs(&mirror.path).await.unwrap().is_empty());
    }
}
