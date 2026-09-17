//! [`GitCredentialProvider`], its v1 PAT implementation and the temporary
//! config file every authenticated command is handed (ADR 0002).
//!
//! **The transport.** A credential is never an argument and never part of a
//! remote URL. The provider hands out an `Authorization: Basic
//! <base64("x-access-token:" + PAT)>` header value; [`CredentialConfig`]
//! writes it into a mode-0600 file under `DATA_DIR/tmp/`, the caller selects
//! that file with [`GitCommand::config_global`](super::GitCommand::config_global)
//! so the child reads it as `GIT_CONFIG_GLOBAL`, and the file is deleted when
//! the guard drops — on success, on failure and on panic
//! (`ARCHITECTURE.md`, "Git model", Credentials). `-c http.extraHeader=…` is
//! explicitly not the transport: those values are argv and any process listing
//! shows them (ADR 0002, superseded note).
//!
//! **The secret.** There is no credential column: a project's credential is
//! the project-scoped, orchestrator-only secret `GIT_CREDENTIAL`
//! (`docs/data-model.md`, `projects`). This module does not read it itself —
//! [`crate::secrets::project_git_credential`] decrypts it and writes the
//! mandatory `secret_uses` row with `purpose = 'git'` in the same transaction,
//! so a credential cannot be obtained without leaving the trace
//! (`docs/data-model.md`, `secret_uses`). A project with no such secret
//! answers `Ok(None)`: the remote is public, the caller runs the command with
//! no config file at all and the remote decides.
//!
//! **Secrets discipline.** The PAT and the header live in `Zeroizing` buffers
//! from the moment they leave the cipher, [`GitCredential`]'s `Debug` is
//! redacted, and no line here carries anything but `project_id` and the secret
//! *name* (`ARCHITECTURE.md`, "Secrets", Credential handling and transcripts;
//! `CLAUDE.md`, rule 3).
//!
//! **Async style.** `#[async_trait::async_trait]`, for the reason given in
//! [`crate::engine`]: the trait is held as `Arc<dyn GitCredentialProvider>`
//! and must stay dyn compatible.

use std::any::Any;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::GitError;
use crate::prelude::*;
use crate::secrets::{GIT_CREDENTIAL_NAME, SecretsKeyring, project_git_credential};

/// Who asked for a credential, as `secret_uses` records it.
///
/// The same three cases the audit table documents — an MCP tool acting for a
/// session, a REST request acting for a user, and the orchestrator's own
/// background work, which is neither (`docs/data-model.md`, `secret_uses`).
/// It is the secrets module's type rather than a parallel enum of this one:
/// the actor is what decides the audit row, and two enums that must agree
/// would eventually not.
pub use crate::secrets::GitUseContext as GitActor;

/// The basic-auth user a GitHub PAT authenticates as.
///
/// GitHub ignores the user half for a token and reads the password half, but
/// the pair must still be well formed; `x-access-token` is the spelling
/// GitHub's own tooling uses (`ARCHITECTURE.md`, "Git model", Credentials).
const PAT_USERNAME: &str = "x-access-token";

/// The value shown instead of a credential in any formatted output (rule 3).
const REDACTED: &str = "<redacted>";

/// The prefix every temporary credential config is named with.
///
/// Fixed so the orphan-cleanup job can recognise one: a `gitcfg-*` left in
/// `DATA_DIR/tmp/` belongs to a process that is gone and is safe to delete.
const CONFIG_PREFIX: &str = "gitcfg-";

/// One credential for upstream git operations.
///
/// It carries the finished header value, not the PAT: whatever a future
/// provider mints — an installation token, an OAuth token — arrives here in
/// the same shape, so callers never learn which implementation they have
/// (ADR 0002).
#[derive(Clone)]
pub struct GitCredential {
    /// The `Authorization:` header value, zeroized when the credential drops.
    /// Private: [`CredentialConfig`] is the only thing that writes it
    /// anywhere.
    header_value: Zeroizing<String>,
    /// When the credential stops working, when that is known. `None` for a
    /// PAT, which has no expiry the orchestrator can see.
    pub expires_at: Option<DateTime<Utc>>,
}

impl GitCredential {
    /// Build the v1 credential from a personal access token.
    ///
    /// The intermediate `x-access-token:<PAT>` buffer is zeroized with the
    /// rest: it is the credential in another encoding.
    ///
    /// A PAT with whitespace in it is refused rather than written: the config
    /// file is a line-oriented format, so a newline would end the value and a
    /// space would change it, and either way the command would authenticate
    /// with something other than what was stored. An empty value is refused
    /// for the same reason it is useless — there is nothing to authenticate
    /// with.
    pub fn from_pat(pat: &str) -> std::result::Result<Self, GitError> {
        if pat.is_empty() || pat.chars().any(char::is_whitespace) {
            // The value never reaches the message; the caller is told only
            // that the stored credential is unusable (rule 3).
            warn!(
                secret = GIT_CREDENTIAL_NAME,
                "the stored git credential is empty or contains whitespace"
            );
            return Err(GitError::CredentialUnavailable);
        }

        let pair = Zeroizing::new(format!("{PAT_USERNAME}:{pat}"));
        let encoded = Zeroizing::new(STANDARD.encode(pair.as_bytes()));

        Ok(Self {
            header_value: Zeroizing::new(format!("Authorization: Basic {}", encoded.as_str())),
            expires_at: None,
        })
    }

    /// Build a credential from an already-formed header value.
    ///
    /// For a provider that mints its own header and for the mock; the PAT path
    /// is [`GitCredential::from_pat`].
    pub fn from_header_value(header_value: Zeroizing<String>) -> Self {
        Self {
            header_value,
            expires_at: None,
        }
    }

    /// The `Authorization:` header value, for [`CredentialConfig`] and for the
    /// tests that assert the encoding.
    pub fn header_value(&self) -> &str {
        self.header_value.as_str()
    }
}

impl fmt::Debug for GitCredential {
    /// Redacted: a credential must not reach a log through a `{:?}` on a
    /// struct that happens to contain one (rule 3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitCredential")
            .field("header_value", &REDACTED)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The name and email commits the orchestrator makes are attributed to.
///
/// `ARCHITECTURE.md`, "Git model", Commit identity: merge commits carry the
/// bot identity from `GIT_BOT_NAME` and `GIT_BOT_EMAIL`, obtained here, so a
/// future GitHub App provider can substitute the app's identity without
/// touching callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdentity {
    /// The committer name.
    pub name: String,
    /// The committer email.
    pub email: String,
}

/// Where every upstream git command gets its credential and its committer
/// identity (ADR 0002).
#[async_trait]
pub trait GitCredentialProvider: Send + Sync {
    /// The credential for this project's upstream, valid for at least
    /// `min_ttl`, or `None` when the project has none and the remote is
    /// public.
    ///
    /// `actor` is written to `secret_uses` by the implementation, so a caller
    /// cannot read a credential without saying who for.
    async fn credential_for(
        &self,
        project_id: Uuid,
        actor: &GitActor,
        min_ttl: Duration,
    ) -> Result<Option<GitCredential>>;

    /// The identity for commits the orchestrator creates itself.
    ///
    /// Per project and fallible because a GitHub App provider resolves it per
    /// installation; the PAT provider answers the configured pair.
    async fn commit_identity(&self, project_id: Uuid) -> Result<CommitIdentity>;

    /// Downcast hook, so a test can read what a handler asked for.
    fn as_any(&self) -> &dyn Any;
}

/// The v1 provider: the project's `GIT_CREDENTIAL` secret as an
/// `Authorization: Basic` header (ADR 0002).
///
/// It holds the pool and the keyring rather than a service handle because the
/// read it makes is one function — the decrypt and its mandatory audit row in
/// one transaction — and the identity because `GIT_BOT_NAME` and
/// `GIT_BOT_EMAIL` are read once at startup.
#[derive(Clone)]
pub struct PatCredentialProvider {
    pool: PgPool,
    keyring: SecretsKeyring,
    identity: CommitIdentity,
}

impl PatCredentialProvider {
    /// Build the provider around the pool, the master keyring and the
    /// configured bot identity.
    pub fn new(pool: PgPool, keyring: SecretsKeyring, identity: CommitIdentity) -> Self {
        Self {
            pool,
            keyring,
            identity,
        }
    }
}

impl fmt::Debug for PatCredentialProvider {
    /// The keyring holds key material, so nothing about it is rendered here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PatCredentialProvider")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl GitCredentialProvider for PatCredentialProvider {
    /// `min_ttl` is unused: a PAT has no expiry the orchestrator can see, so
    /// there is nothing to compare it against and nothing to refresh. The
    /// parameter exists for the GitHub App provider, which refuses or mints a
    /// new installation token when the one it holds is about to lapse
    /// (ADR 0002).
    #[instrument(
        name = "git_credential",
        skip_all,
        fields(project_id = %project_id, secret = GIT_CREDENTIAL_NAME)
    )]
    async fn credential_for(
        &self,
        project_id: Uuid,
        actor: &GitActor,
        _min_ttl: Duration,
    ) -> Result<Option<GitCredential>> {
        // Decrypts and writes the `purpose = 'git'` use row in one
        // transaction: the audit is not this module's to repeat or to skip.
        let Some(pat) =
            project_git_credential(&self.pool, &self.keyring, project_id, *actor).await?
        else {
            return Ok(None);
        };

        Ok(Some(GitCredential::from_pat(&pat)?))
    }

    async fn commit_identity(&self, _project_id: Uuid) -> Result<CommitIdentity> {
        Ok(self.identity.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A temporary `GIT_CONFIG_GLOBAL` carrying one credential.
///
/// The file is created mode-0600 at open time — never created and then
/// chmod-ed, which leaves a window in which it is world-readable — under a
/// name unique to this command, so two concurrent operations on one project
/// never share or reuse a file. Dropping the guard deletes it, which is what
/// makes the file disappear when the command fails or the task panics; a
/// caller that wants to know the deletion succeeded calls
/// [`CredentialConfig::close`].
#[derive(Debug)]
pub struct CredentialConfig {
    path: PathBuf,
    /// Set once the file is gone, so [`Drop`] does not try again after
    /// [`CredentialConfig::close`].
    removed: bool,
}

impl CredentialConfig {
    /// Write `credential` into a fresh mode-0600 file under `tmp_dir`.
    ///
    /// `tmp_dir` is `DATA_DIR/tmp/` in the running orchestrator; it is created
    /// if it is missing, so a fresh data directory needs no separate step.
    pub fn write(
        credential: &GitCredential,
        tmp_dir: &Path,
    ) -> std::result::Result<Self, GitError> {
        std::fs::create_dir_all(tmp_dir)?;

        let path = tmp_dir.join(format!("{CONFIG_PREFIX}{}", Uuid::new_v4()));

        // `create_new` so an existing path is an error rather than a file
        // somebody else's mode applies to, and `mode` so the content is never
        // readable by another user, not even for the instant between the
        // create and a chmod.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;

        // Exactly the two lines git needs. The value is the credential, so
        // nothing about this write is logged (rule 3).
        let contents = Zeroizing::new(format!(
            "[http]\n\textraHeader = {}\n",
            credential.header_value()
        ));

        // A half-written config would make git fail in a way that looks like a
        // credential problem, so the file goes away with the error.
        let written = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);

        if let Err(err) = written {
            let _ = std::fs::remove_file(&path);
            return Err(GitError::Io(err));
        }

        Ok(Self {
            path,
            removed: false,
        })
    }

    /// The file to hand to
    /// [`GitCommand::config_global`](super::GitCommand::config_global).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Delete the file now and say whether it worked.
    ///
    /// The eager half of the pair: [`Drop`] deletes it in any case, but only
    /// this one reports a failure, for a caller that would rather fail an
    /// operation than leave a credential on disk.
    pub fn close(mut self) -> std::result::Result<(), GitError> {
        std::fs::remove_file(&self.path)?;
        self.removed = true;
        Ok(())
    }
}

impl Drop for CredentialConfig {
    /// Best effort by necessity: a destructor has nobody to return an error
    /// to. A failure is worth a line — a credential file left behind is a
    /// finding — and the path is not a secret, the content is.
    fn drop(&mut self) {
        if self.removed {
            return;
        }

        if let Err(err) = std::fs::remove_file(&self.path) {
            warn!(
                path = %self.path.display(),
                error = %err,
                "a temporary git credential config could not be deleted"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::git::GitCommand;

    /// Obviously fake: it authenticates nowhere and never did (rule 3).
    const FAKE_PAT: &str = "ghp_fakefakefake";

    fn fake_credential() -> GitCredential {
        GitCredential::from_pat(FAKE_PAT).expect("an ordinary fake PAT is usable")
    }

    #[test]
    fn a_pat_becomes_the_documented_basic_header() {
        let credential = fake_credential();

        let expected = STANDARD.encode(format!("{PAT_USERNAME}:{FAKE_PAT}").as_bytes());
        assert_eq!(
            credential.header_value(),
            format!("Authorization: Basic {expected}")
        );
        assert!(credential.expires_at.is_none(), "a PAT has no known expiry");
    }

    #[test]
    fn a_pat_with_whitespace_or_no_value_at_all_is_refused() {
        for broken in ["", "ghp_fake token", "ghp_fake\n", "\tghp_fake"] {
            let error =
                GitCredential::from_pat(broken).expect_err("a config-breaking value is refused");
            assert!(
                matches!(error, GitError::CredentialUnavailable),
                "expected CredentialUnavailable, got {error:?}"
            );
        }
    }

    #[test]
    fn debug_never_shows_the_header_or_the_pat() {
        let rendered = format!("{:?}", fake_credential());

        assert!(!rendered.contains(FAKE_PAT), "{rendered}");
        assert!(!rendered.contains("Basic"), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
    }

    #[test]
    fn the_config_is_written_mode_0600_with_exactly_the_two_lines() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let credential = fake_credential();

        let config = CredentialConfig::write(&credential, &dir.path().join("tmp"))
            .expect("the config is written");

        let mode = std::fs::metadata(config.path())
            .expect("the config exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "mode was {mode:04o}");

        let contents = std::fs::read_to_string(config.path()).expect("the config is readable");
        assert_eq!(
            contents,
            format!("[http]\n\textraHeader = {}\n", credential.header_value())
        );
        assert!(
            config
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .expect("the name is UTF-8")
                .starts_with(CONFIG_PREFIX),
            "{:?}",
            config.path()
        );
    }

    #[test]
    fn two_configs_for_one_credential_are_separate_files() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let credential = fake_credential();

        let one = CredentialConfig::write(&credential, dir.path()).expect("the first is written");
        let two = CredentialConfig::write(&credential, dir.path()).expect("the second is written");

        assert_ne!(one.path(), two.path());
        assert!(one.path().exists() && two.path().exists());
    }

    #[test]
    fn dropping_the_guard_deletes_the_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config =
            CredentialConfig::write(&fake_credential(), dir.path()).expect("the config is written");
        let path = config.path().to_path_buf();

        drop(config);

        assert!(!path.exists(), "the credential config outlived its guard");
    }

    #[test]
    fn close_deletes_eagerly_and_drop_does_not_complain_afterwards() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config =
            CredentialConfig::write(&fake_credential(), dir.path()).expect("the config is written");
        let path = config.path().to_path_buf();

        config.close().expect("the file is deleted");

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_failed_command_leaves_no_config_behind_and_no_header_in_its_argv() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let credential = fake_credential();
        let config =
            CredentialConfig::write(&credential, dir.path()).expect("the config is written");
        let path = config.path().to_path_buf();

        // A remote that does not exist: git fails, the failure records the
        // argv, and the guard still has to take the file with it.
        let error = GitCommand::new()
            .args(["fetch", "/nonexistent/mars-test/absent.git"])
            .cwd(dir.path())
            .config_global(config.path())
            .run_ok()
            .await
            .expect_err("fetching a nonexistent remote fails");

        let GitError::Command { ref args, .. } = error else {
            panic!("expected a command failure, got {error:?}");
        };

        for rendered in [format!("{args:?}"), error.to_string(), format!("{error:?}")] {
            assert!(!rendered.contains("Authorization"), "{rendered}");
            assert!(!rendered.contains(FAKE_PAT), "{rendered}");
        }

        drop(config);
        assert!(
            !path.exists(),
            "a failed command left its credential on disk"
        );
    }

    #[tokio::test]
    async fn git_reads_the_header_out_of_the_config_the_provider_wrote() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let credential = fake_credential();
        let config =
            CredentialConfig::write(&credential, dir.path()).expect("the config is written");

        // Not an assertion about the value reaching a remote — that needs one
        // — but about the file being the syntax git accepts at all.
        let output = GitCommand::new()
            .args(["config", "--get", "http.extraHeader"])
            .cwd(dir.path())
            .config_global(config.path())
            .run_ok()
            .await
            .expect("git reads the temporary global config");

        assert_eq!(output.stdout.trim(), credential.header_value());
    }
}
