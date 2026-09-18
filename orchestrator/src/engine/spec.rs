//! Building the [`ContainerSpec`](super::ContainerSpec) for a session or for
//! the startup probe, and translating one into the bollard create body.
//!
//! This module is the single place `ARCHITECTURE.md`, "Session container
//! specification" is turned into code. The launcher supplies only the variable
//! inputs — ids, image, runtime, command, secrets, shared directories — and
//! every fixed value in that table (labels, user, working directory, stdin
//! flags, capabilities, security options, root filesystem, user namespace
//! rule) is decided here and nowhere else. A caller that could vary them could
//! weaken them, so "no `HostConfig` field outside the documented table is
//! used" holds by construction rather than by review: the key-set tests at the
//! bottom fail the moment a field is added without the table being updated
//! first (ADR 0004).
//!
//! [`to_bollard`] is the only function here that names a `bollard` type;
//! everything above it speaks the plain types of [`super::types`].
//!
//! **Secrets.** Resolved secrets enter through [`SessionSpecInput::secrets`]
//! and leave only in [`ContainerSpec::secret_env`], whose [`Debug`] prints keys
//! and never values (CLAUDE.md rule 3). They are `Zeroizing<String>` at both
//! ends and are moved, never reformatted, in between: [`to_bollard`] is the one
//! place the bytes are copied into a plain `String`, and that copy is where the
//! zeroization guarantee ends (`ARCHITECTURE.md`, "Secrets", Resolution at
//! launch). [`SessionSpecInput`] deliberately has no [`Debug`] at all, because
//! [`Zeroizing`]'s own is that of the value it wraps. Nothing here logs, and
//! the one error message names the colliding *variable* — one of five fixed
//! names — and never a value.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
// Shadows the prelude's one-parameter `Result<T>` alias, so this module can
// write the `Result<T, EngineError>` the engine reports.
use std::result::Result;

use bollard::models::{ContainerCreateBody, HostConfig};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::EngineError;
use super::types::{
    Bind, ContainerSpec, EngineKind, LABEL_PROFILE_ID, LABEL_PROJECT_ID, LABEL_SESSION_ID,
    session_container_name,
};
// The crate convention (`CLAUDE.md`, "Backend conventions"). The builder
// reports the engine's own [`EngineError`], so the glob is here for the doc
// links and for what this module grows into.
#[allow(unused_imports)]
use crate::prelude::*;

/// The label marking the startup probe's container, so orphan cleanup can tell
/// one from a session container (`ARCHITECTURE.md`, "Engine adapter", Startup
/// probe).
pub const LABEL_PROBE: &str = "mars.probe";

/// The user every session and probe container runs as: the image's `agent`
/// (`ARCHITECTURE.md`, "Session container specification", Uid contract).
const CONTAINER_USER: &str = "1000:1000";

/// `HostConfig.UsernsMode` on Podman, which maps the host user running Podman
/// to uid 1000 inside the container so files under `DATA_DIR` come back owned
/// by the orchestrator. Docker has no user-namespace mapping and ignores it.
const USERNS_KEEP_ID: &str = "keep-id:uid=1000,gid=1000";

/// The session clone, and the working directory of every container.
const WORK_TARGET: &str = "/session/work";
/// The agent's `HOME`.
const HOME_TARGET: &str = "/session/home";
/// Where the entrypoint redirects the CLI's stdout and stderr.
const LOG_TARGET: &str = "/session/log";
/// The CLI MCP configuration, read-only because it holds the session's bearer
/// token.
const MCP_CONFIG_TARGET: &str = "/session/mcp.json";

/// The environment variables the table fixes, which a project secret may not
/// take over.
///
/// A secret of one of these names would silently change where the agent's
/// `HOME` is, which state directory it shares, or which session it claims to
/// be: the secrets are appended after the fixed environment and the engine
/// takes the last value, so it would quietly win. The builder refuses instead
/// (see [`build_session_spec`]).
pub const RESERVED_ENV_NAMES: [&str; 5] = [
    "HOME",
    "CLAUDE_CONFIG_DIR",
    "MARS_SESSION_ID",
    "MARS_PROJECT_ID",
    "MARS_TASK_ID",
];

/// One of a project's shared directories, as the launcher hands it over
/// (ADR 0015; `ARCHITECTURE.md`, "Storage", Shared directories).
///
/// The `container_path` has already been validated by the project model —
/// absolute, normalised, not under `DATA_DIR`, and neither equal to nor an
/// ancestor of the four session paths. The builder trusts that and only
/// `debug_assert!`s absoluteness, because re-validating here would put the
/// rule in two places and let them disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDirMount {
    /// The directory's name, which is also its directory under
    /// `DATA_DIR_HOST/projects/<pid>/shared/`.
    pub name: String,
    /// Where it is mounted inside the container.
    pub container_path: String,
}

/// Everything that varies between one session container and the next.
///
/// Every field is an input the launcher already has; none of them is a knob
/// over a fixed value in the specification table.
///
/// No [`Debug`], on purpose: [`secrets`](Self::secrets) holds decrypted
/// credentials and [`Zeroizing`]'s own [`Debug`] is that of the `String` it
/// wraps, so a derive here would make `{:?}` on the launcher's input print
/// them. The absent impl is the guarantee, enforced by the compiler rather than
/// by a test (CLAUDE.md rule 3). What a diagnostic wants is the built
/// [`ContainerSpec`], whose [`Debug`] is redacted.
#[derive(Clone)]
pub struct SessionSpecInput {
    /// The session this container runs.
    pub session_id: Uuid,
    /// The session's project, which decides the mirror, the CLI state
    /// directory and the shared directories.
    pub project_id: Uuid,
    /// The agent profile the session was launched with.
    pub profile_id: Uuid,
    /// The task the session was launched for, when it was launched for one.
    /// `None` omits `MARS_TASK_ID` entirely rather than setting it empty.
    pub task_id: Option<Uuid>,
    /// `profile.image` (`docs/data-model.md`, `agent_profiles`).
    pub image: String,
    /// `profile.runtime`; `None`, or an empty string from the column, leaves
    /// the engine's default.
    pub runtime: Option<String>,
    /// The backend's launch command. Empty leaves the image's own.
    pub cmd: Vec<String>,
    /// The resolved secrets, in resolution order, appended after the fixed
    /// environment.
    ///
    /// `ResolvedSecrets::env` verbatim: the values arrive zeroizing from
    /// [`crate::secrets::resolve_for_launch`] and are cloned into
    /// [`ContainerSpec::secret_env`] in the same wrapper, so the launcher never
    /// materialises a plain `String` of a credential
    /// (`ARCHITECTURE.md`, "Secrets", Resolution at launch).
    pub secrets: Vec<(String, Zeroizing<String>)>,
    /// The project's shared directories.
    pub shared_dirs: Vec<SharedDirMount>,
    /// `Config::data_dir`: the orchestrator's own view of the volume, which is
    /// also the path the mirror and the CLI state directory are mounted at
    /// inside the container (ADR 0001: the session clone's alternates file
    /// records the mirror's path as the orchestrator saw it, so it must appear
    /// at that same path in the container).
    pub data_dir: PathBuf,
    /// `Config::data_dir_host`: the host path of the same volume, which is
    /// what a bind-mount source has to be. `Config::from_env` resolves it to
    /// an absolute path at startup.
    pub data_dir_host: PathBuf,
    /// `Config::session_network_internal`: the network the container is
    /// created on. The egress network is connected before start.
    pub network_internal: String,
    /// `Config::session_extra_hosts`, already parsed into `host:ip` entries.
    pub extra_hosts: Vec<String>,
}

/// Everything that varies for the startup probe's container.
///
/// The probe gets the same fixed fields a session gets — the same user, the
/// same security options, the same `UsernsMode` — over its own throwaway
/// directory, because that is the only way its outcome says anything about
/// what a session will get (`ARCHITECTURE.md`, "Engine adapter", Startup
/// probe).
#[derive(Debug, Clone)]
pub struct ProbeSpecInput {
    /// The random suffix of this probe run, which names both the container and
    /// the directory. The caller generates it, so this module needs no `rand`.
    pub suffix: String,
    /// The image to probe with, `Config::session_image_default`.
    pub image: String,
    /// The host path of `DATA_DIR_HOST/tmp/probe-<suffix>`, whose `work`,
    /// `home` and `log` subdirectories the caller has created.
    pub probe_dir_host: PathBuf,
    /// The internal sessions network, as a session would get.
    pub network_internal: String,
    /// The configured extra hosts, as a session would get.
    pub extra_hosts: Vec<String>,
}

/// The container specification for one session launch.
///
/// The field-by-field contract is `ARCHITECTURE.md`, "Session container
/// specification"; this function is that table.
///
/// The binds are built in the table's order and then put through
/// [`order_binds`], which is what guarantees the table's "parents before
/// children" for a shared directory mounted inside the work tree.
///
/// # Errors
///
/// [`EngineError::InvalidSpec`] when a resolved secret is named like one of
/// [`RESERVED_ENV_NAMES`]; the launcher puts the message into `sessions.error`.
/// A collision is a configuration error and not a missing capability, which is
/// why it is not [`EngineError::Unsupported`] (`ARCHITECTURE.md`, "Engine
/// adapter", Normalised semantics).
pub fn build_session_spec(input: &SessionSpecInput) -> Result<ContainerSpec, EngineError> {
    debug_assert!(
        input.data_dir_host.is_absolute(),
        "DATA_DIR_HOST is a bind-mount source and must be absolute; \
         Config::from_env() resolves it against the working directory at startup"
    );
    debug_assert!(
        input.data_dir.is_absolute(),
        "DATA_DIR must be absolute; Config::from_env() resolves it at startup"
    );

    for (name, _) in &input.secrets {
        if RESERVED_ENV_NAMES.contains(&name.as_str()) {
            return Err(EngineError::InvalidSpec(format!(
                "secret name collides with a reserved variable: {name}"
            )));
        }
    }

    let sid = input.session_id;
    let pid = input.project_id;

    let session_host = input.data_dir_host.join("sessions").join(sid.to_string());
    let project_host = input.data_dir_host.join("projects").join(pid.to_string());
    let project_data = input.data_dir.join("projects").join(pid.to_string());

    // `DATA_DIR`-based, not `DATA_DIR_HOST`-based: this is the path inside the
    // container, and the CLI state directory is mounted at the orchestrator's
    // own path (ADR 0001, ADR 0015).
    let claude_config_dir = path_string(&project_data.join("claude"));

    // The fixed variables only; the secrets are a field of their own, appended
    // after these when the adapter builds the engine's environment, so a
    // project secret can never shadow one of them — and the check above means
    // it cannot try.
    let mut env = vec![
        ("HOME".to_string(), HOME_TARGET.to_string()),
        ("CLAUDE_CONFIG_DIR".to_string(), claude_config_dir.clone()),
        ("MARS_SESSION_ID".to_string(), sid.to_string()),
        ("MARS_PROJECT_ID".to_string(), pid.to_string()),
    ];
    if let Some(task_id) = input.task_id {
        env.push(("MARS_TASK_ID".to_string(), task_id.to_string()));
    }

    let mut binds = vec![
        rw(session_host.join("work"), WORK_TARGET),
        rw(session_host.join("home"), HOME_TARGET),
        rw(session_host.join("log"), LOG_TARGET),
        ro(session_host.join("mcp.json"), MCP_CONFIG_TARGET),
        ro(
            project_host.join("repo.git"),
            path_string(&project_data.join("repo.git")),
        ),
        rw(project_host.join("claude"), claude_config_dir),
    ];
    for shared in &input.shared_dirs {
        debug_assert!(
            Path::new(&shared.container_path).is_absolute(),
            "a shared directory's container path is absolute; the project model validates it"
        );
        binds.push(rw(
            project_host.join("shared").join(&shared.name),
            shared.container_path.clone(),
        ));
    }
    order_binds(&mut binds);

    Ok(ContainerSpec {
        image: input.image.clone(),
        name: session_container_name(sid),
        labels: BTreeMap::from([
            (LABEL_SESSION_ID.to_string(), sid.to_string()),
            (LABEL_PROJECT_ID.to_string(), pid.to_string()),
            (LABEL_PROFILE_ID.to_string(), input.profile_id.to_string()),
        ]),
        user: CONTAINER_USER.to_string(),
        working_dir: WORK_TARGET.to_string(),
        cmd: input.cmd.clone(),
        env,
        // Cloned inside the `Zeroizing`, so the copy zeroizes with the spec.
        secret_env: input.secrets.clone(),
        binds,
        network: input.network_internal.clone(),
        extra_hosts: input.extra_hosts.clone(),
        runtime: normalise_runtime(input.runtime.as_deref()),
        open_stdin: true,
    })
}

/// The container specification for one startup-probe run.
///
/// It takes no input and produces no output beyond the file it touches, so
/// stdin stays closed and no `MARS_*` variable is set; everything else is what
/// a session gets (`ARCHITECTURE.md`, "Engine adapter", Startup probe).
pub fn build_probe_spec(input: &ProbeSpecInput) -> ContainerSpec {
    debug_assert!(
        input.probe_dir_host.is_absolute(),
        "the probe directory is a bind-mount source and must be absolute"
    );

    let mut binds = vec![
        rw(input.probe_dir_host.join("work"), WORK_TARGET),
        rw(input.probe_dir_host.join("home"), HOME_TARGET),
        rw(input.probe_dir_host.join("log"), LOG_TARGET),
    ];
    order_binds(&mut binds);

    ContainerSpec {
        image: input.image.clone(),
        name: format!("mars-probe-{}", input.suffix),
        labels: BTreeMap::from([(LABEL_PROBE.to_string(), "true".to_string())]),
        user: CONTAINER_USER.to_string(),
        working_dir: WORK_TARGET.to_string(),
        cmd: vec![
            "sh".to_string(),
            "-c".to_string(),
            "touch /session/work/probe-ok".to_string(),
        ],
        env: Vec::new(),
        secret_env: Vec::new(),
        binds,
        network: input.network_internal.clone(),
        extra_hosts: input.extra_hosts.clone(),
        runtime: None,
        open_stdin: false,
    }
}

/// Order bind mounts so that every parent target precedes any child target.
///
/// The engine mounts in list order, so a shared directory at
/// `/session/work/target` must come after the clone at `/session/work` or the
/// clone's mount would hide it (`ARCHITECTURE.md`, "Engine adapter", nested
/// bind mounts). The engine tests verify the nesting on both Docker and
/// Podman.
///
/// The sort is stable, by number of path components ascending and then
/// lexicographically. Depth alone is what guarantees the property — an
/// ancestor always has fewer components than its descendant — and the
/// lexicographic tiebreak only makes the order of two same-depth, therefore
/// necessarily unrelated, targets deterministic, so a spec can be compared
/// against a fixture.
pub fn order_binds(binds: &mut [Bind]) {
    fn depth(bind: &Bind) -> usize {
        Path::new(&bind.container_target).components().count()
    }

    binds.sort_by(|left, right| {
        depth(left)
            .cmp(&depth(right))
            .then_with(|| left.container_target.cmp(&right.container_target))
    });
}

/// Translate a spec into the body of a container create.
///
/// The only function in this module that names a `bollard` type, and the only
/// place `HostConfig` is built. Every field it sets is a row of
/// `ARCHITECTURE.md`, "Session container specification"; everything else is
/// left at the engine's default, which is what `..Default::default()` means
/// here and what the key-set tests hold it to.
///
/// `kind` decides exactly one thing: `UsernsMode: keep-id`, which Podman needs
/// for the uid contract and Docker ignores (ADR 0004).
///
/// **This is where zeroization ends.** `bollard` takes `Env` as owned
/// `Vec<String>`, so the `NAME=value` line of every
/// [`ContainerSpec::secret_env`] entry is a plain `String` this function builds
/// and hands to the engine call, and neither it nor the JSON body `bollard`
/// serialises it into is wiped. The copy is unavoidable — the container process
/// holds the values anyway — and the guarantee the zeroizing types buy is the
/// narrower one that no orchestrator-side buffer outlives the launch call
/// (`ARCHITECTURE.md`, "Secrets", Resolution at launch). The environment is
/// therefore built here and nowhere else, and the fixed variables come first
/// with the secrets appended last, which is the documented order (`ARCHITECTURE.md`,
/// "Session container specification").
pub fn to_bollard(spec: &ContainerSpec, kind: EngineKind) -> ContainerCreateBody {
    let host_config = HostConfig {
        binds: none_if_empty(spec.binds.iter().map(bind_string).collect::<Vec<String>>()),
        // The internal sessions network, at creation, so the container is
        // never attached to the wrong set; the egress network is a second
        // connect before start (`ARCHITECTURE.md`, "Networks").
        network_mode: Some(spec.network.clone()),
        // `None` rather than an empty list: an empty `ExtraHosts` is a value
        // the engine applies, where the absent field leaves its default.
        extra_hosts: none_if_empty(spec.extra_hosts.clone()),
        cap_drop: Some(vec!["ALL".to_string()]),
        security_opt: Some(vec!["no-new-privileges".to_string()]),
        privileged: Some(false),
        // Writable on purpose: the container is disposable and agents install
        // tools into it.
        readonly_rootfs: Some(false),
        runtime: spec.runtime.clone(),
        userns_mode: match kind {
            EngineKind::Podman => Some(USERNS_KEEP_ID.to_string()),
            EngineKind::Docker => None,
        },
        ..Default::default()
    };

    ContainerCreateBody {
        image: Some(spec.image.clone()),
        // Left to the engine, which uses the container id.
        hostname: None,
        labels: Some(
            spec.labels
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<HashMap<String, String>>(),
        ),
        user: Some(spec.user.clone()),
        working_dir: Some(spec.working_dir.clone()),
        cmd: none_if_empty(spec.cmd.clone()),
        env: none_if_empty(
            spec.env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .chain(
                    spec.secret_env
                        .iter()
                        .map(|(key, value)| format!("{key}={}", value.as_str())),
                )
                .collect::<Vec<String>>(),
        ),
        open_stdin: Some(spec.open_stdin),
        // Stdin stays open for the CLI's whole life: the session owner writes
        // every user message through it.
        stdin_once: Some(false),
        tty: Some(false),
        // Stdin only. Stdout and stderr are read from the transcript file, not
        // the socket (`ARCHITECTURE.md`, "Agent process model").
        attach_stdin: Some(spec.open_stdin),
        attach_stdout: Some(false),
        attach_stderr: Some(false),
        host_config: Some(host_config),
        ..Default::default()
    }
}

/// `<host source>:<container target>:<ro|rw>`, the `Binds` entry format both
/// engines accept. The source is a host path (`DATA_DIR_HOST`) and absolute,
/// which is also what stops Docker reading it as a named volume.
fn bind_string(bind: &Bind) -> String {
    format!(
        "{}:{}:{}",
        bind.host_source.display(),
        bind.container_target,
        if bind.read_only { "ro" } else { "rw" }
    )
}

/// A read-write bind.
fn rw(host_source: PathBuf, container_target: impl Into<String>) -> Bind {
    Bind {
        host_source,
        container_target: container_target.into(),
        read_only: false,
    }
}

/// A read-only bind.
fn ro(host_source: PathBuf, container_target: impl Into<String>) -> Bind {
    Bind {
        host_source,
        container_target: container_target.into(),
        read_only: true,
    }
}

/// A container path as a string. These are built from `DATA_DIR`, which
/// `Config` has already normalised, so there is nothing lossy left to lose.
fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `profile.runtime`, with an empty column value read as "no runtime" rather
/// than as an empty `HostConfig.Runtime` the engine would reject.
fn normalise_runtime(runtime: Option<&str>) -> Option<String> {
    runtime
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// An empty collection is the engine's default, which is not the same as a
/// field explicitly set to nothing.
fn none_if_empty<T>(values: Vec<T>) -> Option<Vec<T>> {
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const SID: &str = "11111111-1111-4111-8111-111111111111";
    const PID: &str = "22222222-2222-4222-8222-222222222222";
    const FID: &str = "33333333-3333-4333-8333-333333333333";
    const TID: &str = "44444444-4444-4444-8444-444444444444";

    fn uuid(text: &str) -> Uuid {
        Uuid::parse_str(text).expect("a valid uuid")
    }

    /// The launcher's inputs for a fresh session. Every secret value here is
    /// obviously fake (rule 3).
    fn session_input() -> SessionSpecInput {
        SessionSpecInput {
            session_id: uuid(SID),
            project_id: uuid(PID),
            profile_id: uuid(FID),
            task_id: None,
            image: "mars-session-claude:dev".to_string(),
            runtime: None,
            cmd: vec!["claude".to_string(), "--print".to_string()],
            secrets: Vec::new(),
            shared_dirs: vec![SharedDirMount {
                name: "target".to_string(),
                container_path: "/session/work/target".to_string(),
            }],
            data_dir: PathBuf::from("/srv/mars/data"),
            data_dir_host: PathBuf::from("/host/mars/data"),
            network_internal: "mars-sessions".to_string(),
            extra_hosts: vec!["host.containers.internal:host-gateway".to_string()],
        }
    }

    fn probe_input() -> ProbeSpecInput {
        ProbeSpecInput {
            suffix: "abc123".to_string(),
            image: "mars-session-claude:dev".to_string(),
            probe_dir_host: PathBuf::from("/host/mars/data/tmp/probe-abc123"),
            network_internal: "mars-sessions".to_string(),
            extra_hosts: vec!["host.containers.internal:host-gateway".to_string()],
        }
    }

    /// Every variable name the container will get, in the order `to_bollard`
    /// renders them: the fixed environment and then the secrets.
    fn env_keys(spec: &ContainerSpec) -> Vec<&str> {
        spec.env
            .iter()
            .map(|(key, _)| key.as_str())
            .chain(spec.secret_env.iter().map(|(key, _)| key.as_str()))
            .collect()
    }

    /// A secret as the resolver hands it over. Obviously fake (rule 3).
    fn secret(name: &str, value: &str) -> (String, Zeroizing<String>) {
        (name.to_string(), Zeroizing::new(value.to_string()))
    }

    fn targets(spec: &ContainerSpec) -> Vec<&str> {
        spec.binds
            .iter()
            .map(|bind| bind.container_target.as_str())
            .collect()
    }

    fn json_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("a JSON object")
            .keys()
            .cloned()
            .collect()
    }

    fn keys(names: [&str; 8]) -> BTreeSet<String> {
        names.into_iter().map(str::to_string).collect()
    }

    /// The whole specification table in one assertion: a literal
    /// [`ContainerSpec`] the table can be read off.
    #[test]
    fn a_fresh_session_is_exactly_the_specification_table() {
        let spec = build_session_spec(&session_input()).expect("the inputs are valid");

        assert_eq!(
            spec,
            ContainerSpec {
                image: "mars-session-claude:dev".to_string(),
                name: format!("mars-session-{SID}"),
                labels: BTreeMap::from([
                    (LABEL_SESSION_ID.to_string(), SID.to_string()),
                    (LABEL_PROJECT_ID.to_string(), PID.to_string()),
                    (LABEL_PROFILE_ID.to_string(), FID.to_string()),
                ]),
                user: "1000:1000".to_string(),
                working_dir: "/session/work".to_string(),
                cmd: vec!["claude".to_string(), "--print".to_string()],
                env: vec![
                    ("HOME".to_string(), "/session/home".to_string()),
                    (
                        "CLAUDE_CONFIG_DIR".to_string(),
                        format!("/srv/mars/data/projects/{PID}/claude"),
                    ),
                    ("MARS_SESSION_ID".to_string(), SID.to_string()),
                    ("MARS_PROJECT_ID".to_string(), PID.to_string()),
                ],
                secret_env: Vec::new(),
                binds: vec![
                    rw(
                        PathBuf::from(format!("/host/mars/data/sessions/{SID}/home")),
                        "/session/home",
                    ),
                    rw(
                        PathBuf::from(format!("/host/mars/data/sessions/{SID}/log")),
                        "/session/log",
                    ),
                    ro(
                        PathBuf::from(format!("/host/mars/data/sessions/{SID}/mcp.json")),
                        "/session/mcp.json",
                    ),
                    rw(
                        PathBuf::from(format!("/host/mars/data/sessions/{SID}/work")),
                        "/session/work",
                    ),
                    rw(
                        PathBuf::from(format!("/host/mars/data/projects/{PID}/shared/target")),
                        "/session/work/target",
                    ),
                    rw(
                        PathBuf::from(format!("/host/mars/data/projects/{PID}/claude")),
                        format!("/srv/mars/data/projects/{PID}/claude"),
                    ),
                    ro(
                        PathBuf::from(format!("/host/mars/data/projects/{PID}/repo.git")),
                        format!("/srv/mars/data/projects/{PID}/repo.git"),
                    ),
                ],
                network: "mars-sessions".to_string(),
                extra_hosts: vec!["host.containers.internal:host-gateway".to_string()],
                runtime: None,
                open_stdin: true,
            }
        );
    }

    /// The nested shared directory is mounted after the clone it sits inside,
    /// which is the property `order_binds` exists for.
    #[test]
    fn a_shared_directory_inside_the_work_tree_follows_the_clone() {
        let spec = build_session_spec(&session_input()).expect("the inputs are valid");
        let targets = targets(&spec);

        let work = targets
            .iter()
            .position(|target| *target == "/session/work")
            .expect("the clone is mounted");
        let shared = targets
            .iter()
            .position(|target| *target == "/session/work/target")
            .expect("the shared directory is mounted");
        assert!(work < shared, "a parent after its child: {targets:?}");
    }

    #[test]
    fn a_session_launched_for_a_task_carries_the_task_id_and_one_launched_without_omits_it() {
        let without = build_session_spec(&session_input()).expect("the inputs are valid");
        assert_eq!(
            env_keys(&without),
            vec![
                "HOME",
                "CLAUDE_CONFIG_DIR",
                "MARS_SESSION_ID",
                "MARS_PROJECT_ID",
            ],
            "MARS_TASK_ID must be absent, not empty"
        );

        let mut input = session_input();
        input.task_id = Some(uuid(TID));
        let with = build_session_spec(&input).expect("the inputs are valid");
        assert_eq!(
            with.env.last(),
            Some(&("MARS_TASK_ID".to_string(), TID.to_string()))
        );
    }

    #[test]
    fn the_resolved_secrets_follow_the_fixed_environment_in_the_order_given() {
        let mut input = session_input();
        input.task_id = Some(uuid(TID));
        input.secrets = vec![
            secret("ANTHROPIC_API_KEY", "not-a-real-key"),
            secret("GH_TOKEN", "not-a-real-token"),
        ];

        let spec = build_session_spec(&input).expect("the inputs are valid");

        assert_eq!(
            env_keys(&spec),
            vec![
                "HOME",
                "CLAUDE_CONFIG_DIR",
                "MARS_SESSION_ID",
                "MARS_PROJECT_ID",
                "MARS_TASK_ID",
                "ANTHROPIC_API_KEY",
                "GH_TOKEN",
            ]
        );
        // The fixed half holds no secret and the secret half holds nothing
        // else, which is what makes the order structural rather than a rule.
        assert_eq!(
            spec.secret_env,
            vec![
                secret("ANTHROPIC_API_KEY", "not-a-real-key"),
                secret("GH_TOKEN", "not-a-real-token"),
            ]
        );
    }

    #[test]
    fn a_secret_named_like_a_fixed_variable_is_refused_by_name() {
        for reserved in RESERVED_ENV_NAMES {
            let mut input = session_input();
            input.secrets = vec![secret(reserved, "not-a-real-value")];

            let error = build_session_spec(&input).expect_err("the collision is refused");
            assert!(
                matches!(error, EngineError::InvalidSpec(_)),
                "unexpected: {error:?}"
            );

            let message = error.to_string();
            assert!(
                message.contains(&format!(
                    "secret name collides with a reserved variable: {reserved}"
                )),
                "unhelpful message: {message}"
            );
            assert!(
                !message.contains("not-a-real-value"),
                "leaked a secret value: {message}"
            );
        }
    }

    #[test]
    fn an_empty_runtime_column_is_the_engine_default_and_a_set_one_is_passed_on() {
        let mut input = session_input();
        input.runtime = Some("   ".to_string());
        assert_eq!(
            build_session_spec(&input)
                .expect("the inputs are valid")
                .runtime,
            None
        );

        input.runtime = Some("runsc".to_string());
        assert_eq!(
            build_session_spec(&input)
                .expect("the inputs are valid")
                .runtime,
            Some("runsc".to_string())
        );
    }

    #[test]
    fn order_binds_puts_every_parent_before_its_children() {
        let mut binds = vec![
            rw(PathBuf::from("/host/target"), "/session/work/target"),
            rw(PathBuf::from("/host/work"), "/session/work"),
            rw(PathBuf::from("/host/home"), "/session/home"),
            rw(PathBuf::from("/host/debug"), "/session/work/target/debug"),
        ];

        order_binds(&mut binds);

        let targets: Vec<&str> = binds
            .iter()
            .map(|bind| bind.container_target.as_str())
            .collect();
        assert_eq!(
            targets,
            vec![
                "/session/home",
                "/session/work",
                "/session/work/target",
                "/session/work/target/debug",
            ]
        );
    }

    #[test]
    fn the_probe_gets_the_sessions_fixed_fields_over_its_own_directory() {
        let spec = build_probe_spec(&probe_input());

        assert_eq!(spec.name, "mars-probe-abc123");
        assert_eq!(
            spec.labels,
            BTreeMap::from([(LABEL_PROBE.to_string(), "true".to_string())])
        );
        assert_eq!(spec.user, "1000:1000");
        assert_eq!(spec.working_dir, "/session/work");
        assert_eq!(spec.network, "mars-sessions");
        assert_eq!(
            spec.extra_hosts,
            vec!["host.containers.internal:host-gateway".to_string()]
        );
        assert_eq!(spec.runtime, None);
        assert!(!spec.open_stdin, "the probe takes no input");
        assert!(spec.env.is_empty(), "the probe gets no MARS_* environment");
        assert!(spec.secret_env.is_empty(), "the probe gets no secrets");
        assert_eq!(
            spec.cmd,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "touch /session/work/probe-ok".to_string(),
            ]
        );
        assert_eq!(
            spec.binds,
            vec![
                rw(
                    PathBuf::from("/host/mars/data/tmp/probe-abc123/home"),
                    "/session/home",
                ),
                rw(
                    PathBuf::from("/host/mars/data/tmp/probe-abc123/log"),
                    "/session/log",
                ),
                rw(
                    PathBuf::from("/host/mars/data/tmp/probe-abc123/work"),
                    "/session/work",
                ),
            ]
        );
    }

    /// The enforcement of "no `HostConfig` field outside the documented table
    /// is used": a field added here without the table being updated first
    /// fails this test (`ARCHITECTURE.md`, "Session container specification").
    #[test]
    fn the_host_config_carries_exactly_the_documented_fields() {
        let spec = build_session_spec(&session_input()).expect("the inputs are valid");

        let host_config = to_bollard(&spec, EngineKind::Podman)
            .host_config
            .expect("a host config");
        let json = serde_json::to_value(&host_config).expect("a host config serialises");

        assert_eq!(
            json_keys(&json),
            keys([
                "Binds",
                "CapDrop",
                "ExtraHosts",
                "NetworkMode",
                "Privileged",
                "ReadonlyRootfs",
                "SecurityOpt",
                "UsernsMode",
            ]),
            "a HostConfig field outside the specification table: {json}"
        );

        assert_eq!(
            host_config.binds,
            Some(vec![
                format!("/host/mars/data/sessions/{SID}/home:/session/home:rw"),
                format!("/host/mars/data/sessions/{SID}/log:/session/log:rw"),
                format!("/host/mars/data/sessions/{SID}/mcp.json:/session/mcp.json:ro"),
                format!("/host/mars/data/sessions/{SID}/work:/session/work:rw"),
                format!("/host/mars/data/projects/{PID}/shared/target:/session/work/target:rw"),
                format!(
                    "/host/mars/data/projects/{PID}/claude:/srv/mars/data/projects/{PID}/claude:rw"
                ),
                format!(
                    "/host/mars/data/projects/{PID}/repo.git:/srv/mars/data/projects/{PID}/repo.git:ro"
                ),
            ])
        );
        assert_eq!(host_config.network_mode, Some("mars-sessions".to_string()));
        assert_eq!(
            host_config.extra_hosts,
            Some(vec!["host.containers.internal:host-gateway".to_string()])
        );
        assert_eq!(host_config.cap_drop, Some(vec!["ALL".to_string()]));
        assert_eq!(
            host_config.security_opt,
            Some(vec!["no-new-privileges".to_string()])
        );
        assert_eq!(host_config.privileged, Some(false));
        assert_eq!(host_config.readonly_rootfs, Some(false));
        assert_eq!(host_config.runtime, None);
    }

    /// `Runtime` appears only when the profile sets one, and adds nothing else.
    #[test]
    fn a_profile_runtime_adds_the_runtime_field_and_nothing_else() {
        let mut input = session_input();
        input.runtime = Some("runsc".to_string());
        let spec = build_session_spec(&input).expect("the inputs are valid");

        let host_config = to_bollard(&spec, EngineKind::Docker)
            .host_config
            .expect("a host config");
        let json = serde_json::to_value(&host_config).expect("a host config serialises");

        assert_eq!(host_config.runtime, Some("runsc".to_string()));
        assert_eq!(
            json_keys(&json),
            // Docker, so `UsernsMode` is absent and `Runtime` is the eighth.
            keys([
                "Binds",
                "CapDrop",
                "ExtraHosts",
                "NetworkMode",
                "Privileged",
                "ReadonlyRootfs",
                "Runtime",
                "SecurityOpt",
            ]),
            "{json}"
        );
    }

    #[test]
    fn keep_id_is_set_on_podman_and_left_off_on_docker() {
        let spec = build_session_spec(&session_input()).expect("the inputs are valid");

        let podman = to_bollard(&spec, EngineKind::Podman)
            .host_config
            .expect("a host config");
        assert_eq!(
            podman.userns_mode,
            Some("keep-id:uid=1000,gid=1000".to_string())
        );

        let docker = to_bollard(&spec, EngineKind::Docker)
            .host_config
            .expect("a host config");
        assert_eq!(docker.userns_mode, None);
        let json = serde_json::to_value(&docker).expect("a host config serialises");
        assert!(
            !json_keys(&json).contains("UsernsMode"),
            "Docker must not be sent keep-id: {json}"
        );
    }

    #[test]
    fn no_extra_hosts_leaves_the_engine_default_untouched() {
        let mut input = session_input();
        input.extra_hosts = Vec::new();
        let spec = build_session_spec(&input).expect("the inputs are valid");

        let host_config = to_bollard(&spec, EngineKind::Docker)
            .host_config
            .expect("a host config");
        assert_eq!(host_config.extra_hosts, None);
        let json = serde_json::to_value(&host_config).expect("a host config serialises");
        assert!(!json_keys(&json).contains("ExtraHosts"), "{json}");
    }

    /// The same enforcement for the create body itself.
    #[test]
    fn the_create_body_carries_exactly_the_documented_fields() {
        let mut input = session_input();
        input.secrets = vec![secret("ANTHROPIC_API_KEY", "not-a-real-key")];
        let spec = build_session_spec(&input).expect("the inputs are valid");

        let body = to_bollard(&spec, EngineKind::Podman);
        let json = serde_json::to_value(&body).expect("a create body serialises");

        let expected: BTreeSet<String> = [
            "AttachStderr",
            "AttachStdin",
            "AttachStdout",
            "Cmd",
            "Env",
            "HostConfig",
            "Image",
            "Labels",
            "OpenStdin",
            "StdinOnce",
            "Tty",
            "User",
            "WorkingDir",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            json_keys(&json),
            expected,
            "a create-body field outside the specification table"
        );

        assert_eq!(body.image, Some("mars-session-claude:dev".to_string()));
        assert_eq!(body.hostname, None);
        assert_eq!(body.user, Some("1000:1000".to_string()));
        assert_eq!(body.working_dir, Some("/session/work".to_string()));
        assert_eq!(
            body.cmd,
            Some(vec!["claude".to_string(), "--print".to_string()])
        );
        assert_eq!(body.open_stdin, Some(true));
        assert_eq!(body.stdin_once, Some(false));
        assert_eq!(body.tty, Some(false));
        assert_eq!(body.attach_stdin, Some(true));
        assert_eq!(body.attach_stdout, Some(false));
        assert_eq!(body.attach_stderr, Some(false));
        assert_eq!(
            body.labels,
            Some(HashMap::from([
                (LABEL_SESSION_ID.to_string(), SID.to_string()),
                (LABEL_PROJECT_ID.to_string(), PID.to_string()),
                (LABEL_PROFILE_ID.to_string(), FID.to_string()),
            ]))
        );
        assert_eq!(
            body.env,
            Some(vec![
                "HOME=/session/home".to_string(),
                format!("CLAUDE_CONFIG_DIR=/srv/mars/data/projects/{PID}/claude"),
                format!("MARS_SESSION_ID={SID}"),
                format!("MARS_PROJECT_ID={PID}"),
                "ANTHROPIC_API_KEY=not-a-real-key".to_string(),
            ])
        );
    }

    /// The one place the resolved values become plain bytes, and the position
    /// they take when they do (`ARCHITECTURE.md`, "Session container
    /// specification", Environment).
    ///
    /// The other half of the guarantee is not assertable at runtime and is not
    /// meant to be: [`SessionSpecInput`] has no [`Debug`] impl, so a
    /// `format!("{:?}", input)` anywhere would not compile, and
    /// [`ContainerSpec`]'s own is redacted (asserted in `engine::types`).
    #[test]
    fn to_bollard_is_where_the_secrets_become_bytes_and_it_appends_them_last() {
        let mut input = session_input();
        input.task_id = Some(uuid(TID));
        input.secrets = vec![
            secret("ANTHROPIC_API_KEY", "not-a-real-key"),
            secret("GH_TOKEN", "not-a-real-token"),
        ];
        let spec = build_session_spec(&input).expect("the inputs are valid");

        let env = to_bollard(&spec, EngineKind::Podman)
            .env
            .expect("a session has an environment");

        assert_eq!(
            env,
            vec![
                "HOME=/session/home".to_string(),
                format!("CLAUDE_CONFIG_DIR=/srv/mars/data/projects/{PID}/claude"),
                format!("MARS_SESSION_ID={SID}"),
                format!("MARS_PROJECT_ID={PID}"),
                format!("MARS_TASK_ID={TID}"),
                "ANTHROPIC_API_KEY=not-a-real-key".to_string(),
                "GH_TOKEN=not-a-real-token".to_string(),
            ],
            "the secrets must be last, in resolution order"
        );

        // And nowhere else in the body: the environment is the only field the
        // values reach.
        let json = serde_json::to_string(&to_bollard(&spec, EngineKind::Podman))
            .expect("a create body serialises");
        assert_eq!(
            json.matches("not-a-real-key").count(),
            1,
            "a secret value appears outside Env: {json}"
        );
    }

    /// The probe is created with the same fixed `HostConfig` a session gets,
    /// which is the whole reason its outcome says anything about sessions.
    #[test]
    fn the_probe_body_keeps_the_sessions_fixed_host_config() {
        let mut input = probe_input();
        input.extra_hosts = Vec::new();
        let probe = build_probe_spec(&input);

        let body = to_bollard(&probe, EngineKind::Podman);
        let host_config = body.host_config.expect("a host config");

        assert_eq!(
            host_config.userns_mode,
            Some("keep-id:uid=1000,gid=1000".to_string())
        );
        assert_eq!(host_config.cap_drop, Some(vec!["ALL".to_string()]));
        assert_eq!(
            host_config.security_opt,
            Some(vec!["no-new-privileges".to_string()])
        );
        assert_eq!(host_config.privileged, Some(false));
        assert_eq!(host_config.readonly_rootfs, Some(false));
        assert_eq!(body.user, Some("1000:1000".to_string()));
        assert_eq!(body.open_stdin, Some(false));
        assert_eq!(body.attach_stdin, Some(false));
        assert_eq!(body.env, None, "the probe has no environment to set");
    }

    #[test]
    fn a_bind_renders_as_the_engines_source_target_mode_triple() {
        assert_eq!(
            bind_string(&ro(PathBuf::from("/host/mars/data/x"), "/session/mcp.json")),
            "/host/mars/data/x:/session/mcp.json:ro"
        );
        assert_eq!(
            bind_string(&rw(PathBuf::from("/host/mars/data/y"), "/session/work")),
            "/host/mars/data/y:/session/work:rw"
        );
    }
}
