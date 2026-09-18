//! Process configuration: one typed field per variable in `README.md`,
//! "Configuration".
//!
//! [`Config::from_env`] is the only place the orchestrator reads the process
//! environment; every other module takes the values from the `Arc<Config>` in
//! `AppState` (`ARCHITECTURE.md`, "Orchestrator internals"). It loads `.env`,
//! applies the documented defaults and fails fast with an error that names
//! every missing required variable at once.
//!
//! The parsing itself lives in [`Config::from_vars`], which takes a lookup
//! closure instead of touching the environment, so unit tests never mutate
//! process-global state. `from_vars` reads one piece of ambient state,
//! `std::env::current_dir()`, to make `DATA_DIR` and `DATA_DIR_HOST`
//! absolute; the pure function behind it, `from_vars_in`, takes that
//! directory as a parameter.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use crate::prelude::debug;

/// What secret-valued `Debug` fields print instead of their value (rule 3).
const REDACTED: &str = "<redacted>";

/// The default value of `SESSION_IMAGE_DEFAULT`: the `latest` alias of the
/// image built from `images/claude/` (`README.md`, "Session image").
const SESSION_IMAGE_DEFAULT: &str = "mars-session-claude:latest";

/// Where the secrets master keyring is read from.
///
/// Exactly one of `SECRETS_MASTER_KEYS` and `SECRETS_MASTER_KEY_FILE` is set.
/// The raw text is kept as given; parsing `<version>=<base64 32 bytes>` entries
/// into a keyring belongs to the secrets manager (`ARCHITECTURE.md`,
/// "Secrets").
#[derive(Clone, PartialEq, Eq)]
pub enum SecretsMasterKeySource {
    /// The literal value of `SECRETS_MASTER_KEYS`.
    Inline(String),
    /// The path in `SECRETS_MASTER_KEY_FILE`; the file holds the same content.
    File(PathBuf),
}

impl fmt::Debug for SecretsMasterKeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(_) => write!(f, "Inline({REDACTED})"),
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
        }
    }
}

/// Why configuration could not be loaded.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Required variables that are unset or empty, in `README.md` table order.
    #[error("missing required configuration: {}", .0.join(", "))]
    Missing(Vec<String>),
    /// A variable is set but its value cannot be used.
    #[error("invalid configuration {name}: {reason}")]
    Invalid { name: String, reason: String },
    /// Both master key sources are set.
    #[error("set exactly one of SECRETS_MASTER_KEYS and SECRETS_MASTER_KEY_FILE")]
    ConflictingKeys,
}

impl ConfigError {
    fn invalid(name: &str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            name: name.to_string(),
            reason: reason.into(),
        }
    }
}

/// Every setting the orchestrator reads from its environment.
///
/// `POSTGRES_USER`, `POSTGRES_PASSWORD` and `POSTGRES_DB` are compose bootstrap
/// variables: they appear in `.env.example` and in the `README.md` table but
/// the orchestrator never reads them, it only uses `DATABASE_URL`.
#[derive(Clone)]
pub struct Config {
    /// The URL users open, used for cookies and email links; no trailing slash.
    pub public_url: String,
    /// Secret for signing access tokens.
    pub jwt_secret: String,
    /// Postgres connection string.
    pub database_url: String,
    /// Engine socket, for example `unix:///run/user/1000/podman/podman.sock`.
    pub docker_host: String,
    /// Host path of the data directory, used as the source of every session
    /// bind mount and therefore resolved to an absolute path at startup: a
    /// relative source is not a path to either engine, and Docker would read
    /// it as a named volume. The directory need not exist.
    pub data_dir_host: PathBuf,
    /// The path at which the orchestrator itself sees the data directory,
    /// resolved to an absolute path at startup. The directory need not exist.
    pub data_dir: PathBuf,
    /// URL written into each session's `mcp.json`.
    pub mcp_url: String,
    /// Name of the internal session network.
    pub session_network_internal: String,
    /// Name of the session egress network.
    pub session_network_egress: String,
    /// Extra `host:ip` entries added to session containers.
    pub session_extra_hosts: Vec<String>,
    /// One or more `<version>=<base64 32-byte key>` entries, or a file holding
    /// them.
    pub secrets_master_keys: SecretsMasterKeySource,
    /// Name for commits the orchestrator creates.
    pub git_bot_name: String,
    /// Email for commits the orchestrator creates.
    pub git_bot_email: String,
    /// Port of the API listener nginx proxies to.
    pub api_port: u16,
    /// Port of the MCP listener on the sessions network.
    pub mcp_port: u16,
    /// Seconds between SIGINT and SIGTERM when stopping a session.
    pub stop_grace_secs: u64,
    /// How often project mirrors are fetched.
    pub mirror_fetch_interval_secs: u64,
    /// Image used by the default profile of new projects and by the startup
    /// probe; defaults to [`SESSION_IMAGE_DEFAULT`].
    pub session_image_default: String,
    /// Resend API key; `None` selects the logging email fallback (ADR 0026).
    pub resend_api_key: Option<String>,
    /// Sender address; required when `resend_api_key` is set.
    pub mail_from: Option<String>,
    /// Log filter handed to the tracing initialiser.
    pub rust_log: String,
}

impl Config {
    /// Load `.env` and build the configuration from the process environment.
    ///
    /// `.env` is looked for in the current working directory first and then at
    /// `../.env`, so `cargo run` from `orchestrator/` picks up the repository
    /// root `.env` the README tells operators to create. `dotenvy` never
    /// overrides a variable already present in the environment.
    pub fn from_env() -> std::result::Result<Self, ConfigError> {
        Self::load_dotenv();
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn load_dotenv() {
        for candidate in [".env", "../.env"] {
            match dotenvy::from_filename(candidate) {
                Ok(path) => {
                    // The file name only, never any of the values it carries.
                    debug!(env_file = %path.display(), "loaded environment file");
                    return;
                }
                Err(err) if err.not_found() => continue,
                Err(err) => {
                    debug!(env_file = candidate, error = %err, "could not load environment file");
                    return;
                }
            }
        }
        debug!("no .env file found; using the process environment only");
    }

    /// Build the configuration from a lookup closure.
    ///
    /// Pure except for `std::env::current_dir()`, which is needed to make
    /// `DATA_DIR` and `DATA_DIR_HOST` absolute; `from_vars_in` takes that
    /// directory as a parameter.
    pub fn from_vars<F>(vars: F) -> std::result::Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let current_dir = std::env::current_dir().map_err(|err| {
            ConfigError::invalid(
                "DATA_DIR",
                format!("cannot read the current directory: {err}"),
            )
        })?;
        Self::from_vars_in(vars, &current_dir)
    }

    fn from_vars_in<F>(vars: F, current_dir: &Path) -> std::result::Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        // Collected in `README.md` table order so the error reads like the
        // table the operator is filling in.
        let mut missing: Vec<String> = Vec::new();

        let public_url = required(&vars, "PUBLIC_URL", &mut missing);
        let jwt_secret = required(&vars, "JWT_SECRET", &mut missing);
        let database_url = required(&vars, "DATABASE_URL", &mut missing);
        let docker_host = required(&vars, "DOCKER_HOST", &mut missing);
        let data_dir_host = required(&vars, "DATA_DIR_HOST", &mut missing);

        let mut conflicting_keys = false;
        let secrets_master_keys = match (
            value(&vars, "SECRETS_MASTER_KEYS"),
            value(&vars, "SECRETS_MASTER_KEY_FILE"),
        ) {
            (Some(_), Some(_)) => {
                conflicting_keys = true;
                None
            }
            (Some(keys), None) => Some(SecretsMasterKeySource::Inline(keys)),
            (None, Some(path)) => Some(SecretsMasterKeySource::File(PathBuf::from(path))),
            (None, None) => {
                missing.push("SECRETS_MASTER_KEYS or SECRETS_MASTER_KEY_FILE".to_string());
                None
            }
        };

        let git_bot_name = required(&vars, "GIT_BOT_NAME", &mut missing);
        let git_bot_email = required(&vars, "GIT_BOT_EMAIL", &mut missing);

        // `MAIL_FROM` is only required when mail actually goes out; without a
        // key the log fallback is used instead (ADR 0026).
        let resend_api_key = value(&vars, "RESEND_API_KEY");
        let mail_from = if resend_api_key.is_some() {
            required(&vars, "MAIL_FROM", &mut missing)
        } else {
            value(&vars, "MAIL_FROM")
        };

        if !missing.is_empty() {
            return Err(ConfigError::Missing(missing));
        }
        if conflicting_keys {
            return Err(ConfigError::ConflictingKeys);
        }

        // Every `required` above returned `Some`, or `missing` was not empty.
        let public_url = public_url.expect("PUBLIC_URL present");
        let jwt_secret = jwt_secret.expect("JWT_SECRET present");
        let database_url = database_url.expect("DATABASE_URL present");
        let docker_host = docker_host.expect("DOCKER_HOST present");
        let data_dir_host = data_dir_host.expect("DATA_DIR_HOST present");
        let secrets_master_keys = secrets_master_keys.expect("a master key source present");
        let git_bot_name = git_bot_name.expect("GIT_BOT_NAME present");
        let git_bot_email = git_bot_email.expect("GIT_BOT_EMAIL present");

        let public_url = normalise_public_url(&public_url)?;

        // Both views of the volume are made absolute here. `DATA_DIR` is this
        // process's own view. `DATA_DIR_HOST` is a host path the engine
        // interprets, and a relative one is not a path to it at all — Docker
        // reads it as a named volume — so it is resolved against this
        // process's working directory too: a relative value only makes sense
        // when the orchestrator runs on the host, where its working directory
        // *is* the host's, and in compose the value is absolute already and
        // the join is a no-op (`ARCHITECTURE.md`, "Storage"; `README.md`,
        // "Configuration").
        let data_dir = value(&vars, "DATA_DIR").unwrap_or_else(|| "./data".to_string());
        let data_dir = lexically_normalise(&current_dir.join(data_dir));
        let data_dir_host = lexically_normalise(&current_dir.join(data_dir_host));

        let api_port: u16 = optional_parsed(&vars, "API_PORT", 7000)?;
        let mcp_port: u16 = optional_parsed(&vars, "MCP_PORT", 7001)?;
        if api_port == mcp_port {
            return Err(ConfigError::invalid(
                "MCP_PORT",
                "must differ from API_PORT",
            ));
        }

        let mcp_url = value(&vars, "MCP_URL")
            .unwrap_or_else(|| format!("http://orchestrator:{mcp_port}/mcp"));

        let session_network_internal =
            value(&vars, "SESSION_NETWORK_INTERNAL").unwrap_or_else(|| "mars-sessions".to_string());
        let session_network_egress =
            value(&vars, "SESSION_NETWORK_EGRESS").unwrap_or_else(|| "mars-egress".to_string());
        let session_extra_hosts = parse_extra_hosts(value(&vars, "SESSION_EXTRA_HOSTS"))?;

        let stop_grace_secs: u64 = optional_parsed(&vars, "STOP_GRACE_SECS", 20)?;
        let mirror_fetch_interval_secs: u64 =
            optional_parsed(&vars, "MIRROR_FETCH_INTERVAL_SECS", 600)?;

        // Optional so a default installation needs no image name: the value
        // below is the tag the documented build command produces, and both the
        // default profile of a new project and the startup probe pull it
        // (`README.md`, "Configuration"; `ARCHITECTURE.md`, "Session image").
        let session_image_default = value(&vars, "SESSION_IMAGE_DEFAULT")
            .unwrap_or_else(|| SESSION_IMAGE_DEFAULT.to_string());

        let rust_log = value(&vars, "RUST_LOG").unwrap_or_else(|| "info".to_string());

        Ok(Self {
            public_url,
            jwt_secret,
            database_url,
            docker_host,
            data_dir_host,
            data_dir,
            mcp_url,
            session_network_internal,
            session_network_egress,
            session_extra_hosts,
            secrets_master_keys,
            git_bot_name,
            git_bot_email,
            api_port,
            mcp_port,
            stop_grace_secs,
            mirror_fetch_interval_secs,
            session_image_default,
            resend_api_key,
            mail_from,
            rust_log,
        })
    }

    /// Whether `PUBLIC_URL` is https, which decides the `Secure` flag on the
    /// refresh-token cookie (`SPEC.md`, "Authentication").
    pub fn public_url_is_https(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    /// Where `project_id` keeps its repository, its CLI state and its shared
    /// directories, in both views of the data volume
    /// ([`ProjectLayout`](crate::projects::ProjectLayout);
    /// `ARCHITECTURE.md`, "Storage").
    ///
    /// The one place production code builds a layout: this is the only type
    /// that holds both `DATA_DIR` and `DATA_DIR_HOST`.
    pub fn project_layout(&self, project_id: uuid::Uuid) -> crate::projects::ProjectLayout {
        crate::projects::ProjectLayout::new_with_host(
            &self.data_dir,
            &self.data_dir_host,
            project_id,
        )
    }
}

impl fmt::Debug for Config {
    /// Written by hand so no secret value can reach a log line through a `{:?}`
    /// of the configuration (rule 3 of `CLAUDE.md`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("public_url", &self.public_url)
            .field("jwt_secret", &REDACTED)
            .field("database_url", &REDACTED)
            .field("docker_host", &self.docker_host)
            .field("data_dir_host", &self.data_dir_host)
            .field("data_dir", &self.data_dir)
            .field("mcp_url", &self.mcp_url)
            .field("session_network_internal", &self.session_network_internal)
            .field("session_network_egress", &self.session_network_egress)
            .field("session_extra_hosts", &self.session_extra_hosts)
            .field("secrets_master_keys", &REDACTED)
            .field("git_bot_name", &self.git_bot_name)
            .field("git_bot_email", &self.git_bot_email)
            .field("api_port", &self.api_port)
            .field("mcp_port", &self.mcp_port)
            .field("stop_grace_secs", &self.stop_grace_secs)
            .field(
                "mirror_fetch_interval_secs",
                &self.mirror_fetch_interval_secs,
            )
            .field("session_image_default", &self.session_image_default)
            .field(
                "resend_api_key",
                &self.resend_api_key.as_ref().map(|_| REDACTED),
            )
            .field("mail_from", &self.mail_from)
            .field("rust_log", &self.rust_log)
            .finish()
    }
}

/// A trimmed value, treating an unset variable and an empty one alike: a `.env`
/// line such as `JWT_SECRET=` is a common mistake and must not pass as set.
fn value<F>(vars: &F, name: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    vars(name)
        .map(|raw| raw.trim().to_string())
        .filter(|trimmed| !trimmed.is_empty())
}

/// A required value; records the name instead of returning early so that one
/// run reports every missing variable.
fn required<F>(vars: &F, name: &str, missing: &mut Vec<String>) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let found = value(vars, name);
    if found.is_none() {
        missing.push(name.to_string());
    }
    found
}

fn optional_parsed<F, T>(vars: &F, name: &str, default: T) -> std::result::Result<T, ConfigError>
where
    F: Fn(&str) -> Option<String>,
    T: FromStr,
    T::Err: fmt::Display,
{
    match value(vars, name) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<T>()
            .map_err(|err| ConfigError::invalid(name, err.to_string())),
    }
}

/// Validate the scheme and strip trailing slashes so the auth epic can append
/// paths to build links.
fn normalise_public_url(raw: &str) -> std::result::Result<String, ConfigError> {
    let scheme_len = if let Some(rest) = raw.strip_prefix("https://") {
        raw.len() - rest.len()
    } else if let Some(rest) = raw.strip_prefix("http://") {
        raw.len() - rest.len()
    } else {
        return Err(ConfigError::invalid(
            "PUBLIC_URL",
            "must start with http:// or https://",
        ));
    };

    let trimmed = raw.trim_end_matches('/');
    if trimmed.len() <= scheme_len {
        return Err(ConfigError::invalid("PUBLIC_URL", "must include a host"));
    }
    Ok(trimmed.to_string())
}

/// `host:ip` entries, comma separated; blank entries are dropped so a trailing
/// comma is harmless.
fn parse_extra_hosts(raw: Option<String>) -> std::result::Result<Vec<String>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if entry.matches(':').count() != 1 {
            return Err(ConfigError::invalid(
                "SESSION_EXTRA_HOSTS",
                format!("entry `{entry}` must be host:ip"),
            ));
        }
        entries.push(entry.to_string());
    }
    Ok(entries)
}

/// Resolve `.` and `..` without touching the filesystem, so the data directory
/// can be made absolute before it exists.
fn lexically_normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if out.parent().is_some() {
                    out.pop();
                } else if out.as_os_str().is_empty() {
                    out.push("..");
                }
                // At the root, `..` is the root; drop it.
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    /// The `README.md` "Configuration" table, in table order. This list is the
    /// contract: `.env.example` carries exactly these names.
    const README_VARIABLES: &[&str] = &[
        "PUBLIC_URL",
        "JWT_SECRET",
        "DATABASE_URL",
        "POSTGRES_USER",
        "POSTGRES_PASSWORD",
        "POSTGRES_DB",
        "DOCKER_HOST",
        "DATA_DIR_HOST",
        "DATA_DIR",
        "MCP_URL",
        "SESSION_NETWORK_INTERNAL",
        "SESSION_NETWORK_EGRESS",
        "SESSION_EXTRA_HOSTS",
        "SECRETS_MASTER_KEYS",
        "GIT_BOT_NAME",
        "GIT_BOT_EMAIL",
        "API_PORT",
        "MCP_PORT",
        "STOP_GRACE_SECS",
        "MIRROR_FETCH_INTERVAL_SECS",
        "SESSION_IMAGE_DEFAULT",
        "RESEND_API_KEY",
        "MAIL_FROM",
        "RUST_LOG",
    ];

    /// A fixed directory to resolve `DATA_DIR` against, so assertions do not
    /// depend on where the test runner was started.
    const BASE: &str = "/srv/mars";

    fn required_only() -> HashMap<String, String> {
        [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "unix:///run/user/1000/podman/podman.sock"),
            ("DATA_DIR_HOST", "/srv/mars/data"),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn load(vars: &HashMap<String, String>) -> std::result::Result<Config, ConfigError> {
        Config::from_vars_in(|name| vars.get(name).cloned(), Path::new(BASE))
    }

    #[test]
    fn required_values_plus_documented_defaults() {
        let config = load(&required_only()).expect("a complete required set loads");

        assert_eq!(config.public_url, "https://mars.example.invalid");
        assert!(config.public_url_is_https());
        assert_eq!(config.jwt_secret, "not-a-real-signing-secret");
        assert_eq!(
            config.database_url,
            "postgres://mars:fake@localhost:5432/mars"
        );
        assert_eq!(
            config.docker_host,
            "unix:///run/user/1000/podman/podman.sock"
        );
        assert_eq!(config.data_dir_host, PathBuf::from("/srv/mars/data"));
        assert_eq!(
            config.secrets_master_keys,
            SecretsMasterKeySource::Inline("1=not-a-real-key".to_string())
        );
        assert_eq!(config.git_bot_name, "Mars Bot");
        assert_eq!(config.git_bot_email, "mars-bot@example.invalid");

        assert_eq!(config.data_dir, PathBuf::from("/srv/mars/data"));
        assert_eq!(config.mcp_url, "http://orchestrator:7001/mcp");
        assert_eq!(config.session_network_internal, "mars-sessions");
        assert_eq!(config.session_network_egress, "mars-egress");
        assert!(config.session_extra_hosts.is_empty());
        assert_eq!(config.api_port, 7000);
        assert_eq!(config.mcp_port, 7001);
        assert_eq!(config.stop_grace_secs, 20);
        assert_eq!(config.mirror_fetch_interval_secs, 600);
        assert_eq!(config.session_image_default, "mars-session-claude:latest");
        assert_eq!(config.resend_api_key, None);
        assert_eq!(config.mail_from, None);
        assert_eq!(config.rust_log, "info");
    }

    #[test]
    fn data_dir_is_made_absolute_and_normalised() {
        let mut vars = required_only();
        vars.insert("DATA_DIR".to_string(), "./var/../data".to_string());
        let config = load(&vars).expect("loads");
        assert_eq!(config.data_dir, PathBuf::from("/srv/mars/data"));

        vars.insert("DATA_DIR".to_string(), "/mnt/mars/data".to_string());
        let config = load(&vars).expect("loads");
        assert_eq!(config.data_dir, PathBuf::from("/mnt/mars/data"));
    }

    /// A bind-mount source has to be an absolute path, so a relative
    /// `DATA_DIR_HOST` — which only makes sense when the orchestrator runs on
    /// the host — is resolved against the working directory, and an absolute
    /// one is left alone (`README.md`, "Configuration").
    #[test]
    fn data_dir_host_is_made_absolute_and_normalised() {
        let mut vars = required_only();
        vars.insert("DATA_DIR_HOST".to_string(), "./var/../data".to_string());
        let config = load(&vars).expect("loads");
        assert_eq!(config.data_dir_host, PathBuf::from("/srv/mars/data"));

        vars.insert("DATA_DIR_HOST".to_string(), "/mnt/mars/data".to_string());
        let config = load(&vars).expect("loads");
        assert_eq!(config.data_dir_host, PathBuf::from("/mnt/mars/data"));
    }

    /// The variable is optional: an installation that built the image under
    /// the documented tag needs no value, and a value still wins
    /// (`README.md`, "Configuration").
    #[test]
    fn session_image_default_is_optional_and_overridable() {
        let config = load(&required_only()).expect("no image name is fine");
        assert_eq!(config.session_image_default, "mars-session-claude:latest");

        let mut vars = required_only();
        vars.insert(
            "SESSION_IMAGE_DEFAULT".to_string(),
            "mars-session-stub:latest".to_string(),
        );
        let config = load(&vars).expect("loads");
        assert_eq!(config.session_image_default, "mars-session-stub:latest");
    }

    #[test]
    fn missing_variables_are_reported_together_in_table_order() {
        let mut vars = required_only();
        for name in ["PUBLIC_URL", "JWT_SECRET", "GIT_BOT_EMAIL"] {
            vars.remove(name);
        }

        let error = load(&vars).expect_err("an incomplete set fails");
        assert_eq!(
            error,
            ConfigError::Missing(vec![
                "PUBLIC_URL".to_string(),
                "JWT_SECRET".to_string(),
                "GIT_BOT_EMAIL".to_string(),
            ])
        );
        assert_eq!(
            error.to_string(),
            "missing required configuration: PUBLIC_URL, JWT_SECRET, GIT_BOT_EMAIL"
        );
    }

    #[test]
    fn an_empty_value_counts_as_missing() {
        let mut vars = required_only();
        vars.insert("JWT_SECRET".to_string(), "   ".to_string());

        assert_eq!(
            load(&vars).expect_err("an empty required value fails"),
            ConfigError::Missing(vec!["JWT_SECRET".to_string()])
        );
    }

    #[test]
    fn neither_master_key_source_is_missing_as_one_entry() {
        let mut vars = required_only();
        vars.remove("SECRETS_MASTER_KEYS");

        let error = load(&vars).expect_err("no key source fails");
        assert_eq!(
            error,
            ConfigError::Missing(vec![
                "SECRETS_MASTER_KEYS or SECRETS_MASTER_KEY_FILE".to_string()
            ])
        );
    }

    #[test]
    fn a_master_key_file_is_accepted() {
        let mut vars = required_only();
        vars.remove("SECRETS_MASTER_KEYS");
        vars.insert(
            "SECRETS_MASTER_KEY_FILE".to_string(),
            "/run/secrets/mars-master-keys".to_string(),
        );

        let config = load(&vars).expect("loads");
        assert_eq!(
            config.secrets_master_keys,
            SecretsMasterKeySource::File(PathBuf::from("/run/secrets/mars-master-keys"))
        );
    }

    #[test]
    fn both_master_key_sources_conflict() {
        let mut vars = required_only();
        vars.insert(
            "SECRETS_MASTER_KEY_FILE".to_string(),
            "/run/secrets/mars-master-keys".to_string(),
        );

        let error = load(&vars).expect_err("two key sources fail");
        assert_eq!(error, ConfigError::ConflictingKeys);
        assert_eq!(
            error.to_string(),
            "set exactly one of SECRETS_MASTER_KEYS and SECRETS_MASTER_KEY_FILE"
        );
    }

    #[test]
    fn public_url_must_carry_a_scheme_and_loses_its_trailing_slash() {
        let mut vars = required_only();
        vars.insert("PUBLIC_URL".to_string(), "mars.example.invalid".to_string());
        assert_eq!(
            load(&vars).expect_err("a schemeless URL fails"),
            ConfigError::Invalid {
                name: "PUBLIC_URL".to_string(),
                reason: "must start with http:// or https://".to_string(),
            }
        );

        vars.insert(
            "PUBLIC_URL".to_string(),
            "http://localhost:8080/".to_string(),
        );
        let config = load(&vars).expect("loads");
        assert_eq!(config.public_url, "http://localhost:8080");
        assert!(!config.public_url_is_https());
    }

    #[test]
    fn the_two_listener_ports_must_differ() {
        let mut vars = required_only();
        vars.insert("API_PORT".to_string(), "7001".to_string());

        assert_eq!(
            load(&vars).expect_err("equal ports fail"),
            ConfigError::Invalid {
                name: "MCP_PORT".to_string(),
                reason: "must differ from API_PORT".to_string(),
            }
        );
    }

    #[test]
    fn the_default_mcp_url_follows_the_configured_mcp_port() {
        let mut vars = required_only();
        vars.insert("MCP_PORT".to_string(), "9001".to_string());

        let config = load(&vars).expect("loads");
        assert_eq!(config.mcp_port, 9001);
        assert_eq!(config.mcp_url, "http://orchestrator:9001/mcp");
    }

    #[test]
    fn session_extra_hosts_are_trimmed_and_validated() {
        let mut vars = required_only();
        vars.insert("SESSION_EXTRA_HOSTS".to_string(), "a:1, b:2 ,".to_string());
        let config = load(&vars).expect("loads");
        assert_eq!(config.session_extra_hosts, vec!["a:1", "b:2"]);

        vars.insert("SESSION_EXTRA_HOSTS".to_string(), "nocolon".to_string());
        assert_eq!(
            load(&vars).expect_err("an entry without a colon fails"),
            ConfigError::Invalid {
                name: "SESSION_EXTRA_HOSTS".to_string(),
                reason: "entry `nocolon` must be host:ip".to_string(),
            }
        );
    }

    #[test]
    fn stop_grace_may_be_zero_but_must_be_a_number() {
        let mut vars = required_only();
        vars.insert("STOP_GRACE_SECS".to_string(), "0".to_string());
        assert_eq!(load(&vars).expect("loads").stop_grace_secs, 0);

        vars.insert("STOP_GRACE_SECS".to_string(), "-1".to_string());
        let error = load(&vars).expect_err("a negative grace period fails");
        assert!(
            matches!(error, ConfigError::Invalid { ref name, .. } if name == "STOP_GRACE_SECS")
        );
        assert!(
            error
                .to_string()
                .starts_with("invalid configuration STOP_GRACE_SECS: ")
        );

        vars.insert("STOP_GRACE_SECS".to_string(), "soon".to_string());
        assert!(matches!(
            load(&vars).expect_err("a non-numeric grace period fails"),
            ConfigError::Invalid { ref name, .. } if name == "STOP_GRACE_SECS"
        ));
    }

    #[test]
    fn a_resend_key_requires_a_sender_address() {
        let mut vars = required_only();
        vars.insert(
            "RESEND_API_KEY".to_string(),
            "re_not_a_real_key".to_string(),
        );
        assert_eq!(
            load(&vars).expect_err("a key without a sender fails"),
            ConfigError::Missing(vec!["MAIL_FROM".to_string()])
        );

        vars.insert("MAIL_FROM".to_string(), "mars@example.invalid".to_string());
        let config = load(&vars).expect("loads");
        assert_eq!(config.resend_api_key.as_deref(), Some("re_not_a_real_key"));
        assert_eq!(config.mail_from.as_deref(), Some("mars@example.invalid"));
    }

    #[test]
    fn an_empty_resend_key_selects_the_log_fallback() {
        let mut vars = required_only();
        vars.insert("RESEND_API_KEY".to_string(), String::new());

        let config = load(&vars).expect("loads");
        assert_eq!(config.resend_api_key, None);
        assert_eq!(config.mail_from, None);
    }

    #[test]
    fn debug_never_prints_a_secret_value() {
        let mut vars = required_only();
        vars.insert("JWT_SECRET".to_string(), "jwt-secret-value".to_string());
        vars.insert(
            "DATABASE_URL".to_string(),
            "postgres://mars:database-password@db/mars".to_string(),
        );
        vars.insert(
            "SECRETS_MASTER_KEYS".to_string(),
            "1=master-key-value".to_string(),
        );
        vars.insert("RESEND_API_KEY".to_string(), "resend-key-value".to_string());
        vars.insert("MAIL_FROM".to_string(), "mars@example.invalid".to_string());

        let config = load(&vars).expect("loads");
        let rendered = format!("{config:?}");

        for secret in [
            "jwt-secret-value",
            "database-password",
            "postgres://",
            "master-key-value",
            "resend-key-value",
        ] {
            assert!(
                !rendered.contains(secret),
                "Debug output leaked {secret}: {rendered}"
            );
        }
        assert!(rendered.contains(REDACTED));
        // Non-secret fields are still useful in a log line.
        assert!(rendered.contains("mars.example.invalid"));
        assert!(format!("{:?}", config.secrets_master_keys).contains(REDACTED));
    }

    #[test]
    fn env_example_carries_exactly_the_readme_variables_and_loads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../.env.example");
        let entries: HashMap<String, String> = dotenvy::from_path_iter(path)
            .expect("`.env.example` is readable")
            .map(|entry| entry.expect("`.env.example` parses"))
            .collect();

        let found: BTreeSet<&str> = entries.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = README_VARIABLES.iter().copied().collect();
        assert_eq!(
            found, expected,
            "`.env.example` must list exactly the README.md \"Configuration\" variables"
        );

        let config = Config::from_vars_in(|name| entries.get(name).cloned(), Path::new(BASE))
            .expect("`.env.example` is a usable configuration");
        // `.env.example` ships the development default `./data` for both, and
        // both are resolved against the working directory at startup.
        assert_eq!(config.data_dir_host, PathBuf::from("/srv/mars/data"));
        assert_eq!(config.data_dir, PathBuf::from("/srv/mars/data"));
        assert_eq!(config.api_port, 7000);
        assert_eq!(config.mcp_port, 7001);
        assert_eq!(config.resend_api_key, None);
        // `.env.example` spells out the same image name the code falls back to,
        // so the README table, the file and this module cannot drift apart.
        assert_eq!(config.session_image_default, SESSION_IMAGE_DEFAULT);
    }
}
