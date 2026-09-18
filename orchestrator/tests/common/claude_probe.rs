//! The live Claude Code probe's plumbing: opt-in gating, a child process with
//! line-by-line stdout, the fixture recorder and the credential-leak scanner.
//!
//! The probe itself is `tests/claude_probe.rs`; everything here is the
//! machinery it would otherwise repeat nine times. Nothing in this module
//! touches Postgres, the engine or `TestApp`: it spawns the real `claude`
//! binary found on `PATH` (`MARS_CLAUDE_BIN` overrides it) in throwaway
//! directories and reads its `stream-json` output.
//!
//! **Opt in, twice.** [`probe_or_skip`] returns `None` — after one line on
//! stderr — unless `MARS_CLAUDE_PROBE=1`, so CI and every ordinary
//! `cargo test` run pass with the probe skipped (`CLAUDE.md`, "Testing
//! expectations"). Recording is a second opt-in, `MARS_RECORD_FIXTURES=1`, and
//! never overwrites a version directory that already exists: a CLI bump adds
//! fixtures, it never edits old ones.
//!
//! **Secrets.** The credential is read from the environment into
//! [`Credential`] and goes no further than the child's environment: it is
//! never logged, never put in a failure message and never written to a
//! fixture. [`scan_lines`] is the guard — the recorder refuses to write
//! anything that contains the credential value, the host's home path or any
//! `sk-ant-` token other than the obviously fake one (`CLAUDE.md`, rule 3).

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use mars_orchestrator::agent::claude::CLAUDE_CLI_VERSION;
use mars_orchestrator::agent::{LaunchContext, LaunchMode};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// The variable that enables the probe at all.
pub const ENABLE_VAR: &str = "MARS_CLAUDE_PROBE";
/// The variable that enables fixture recording.
pub const RECORD_VAR: &str = "MARS_RECORD_FIXTURES";
/// The variable that overrides the CLI path.
pub const BIN_VAR: &str = "MARS_CLAUDE_BIN";
/// The CLI as it is found on `PATH` when [`BIN_VAR`] is unset.
pub const DEFAULT_BIN: &str = "claude";

/// The two credentials a launch may carry (`ARCHITECTURE.md`, "Claude Code
/// invocation", Credentials). Exactly one of them, never both — the same rule
/// the launcher applies.
pub const CREDENTIAL_VARS: [&str; 2] = ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"];

/// The obviously fake key the `auth_failure` scenario runs with (rule 3). It
/// is the one `sk-ant-` string the leak scanner lets through.
pub const FAKE_CREDENTIAL: &str = "sk-ant-fake-probe-0000";

/// The prefix every real Anthropic key shares, and what the scanner hunts for.
const KEY_PREFIX: &str = "sk-ant-";

/// The line printed when the probe is not enabled.
const SKIPPED: &str = "claude probe skipped: MARS_CLAUDE_PROBE unset";

/// How long one scenario may take from its first launch. Every read is bounded
/// by what is left of it, so a CLI that stops talking fails the scenario with
/// the lines seen so far instead of hanging the suite.
pub const SCENARIO_TIMEOUT: Duration = Duration::from_secs(120);

/// The MCP server name the launcher writes into every session's `mcp.json`
/// (`ARCHITECTURE.md`, "Claude Code invocation").
pub const MCP_SERVER_NAME: &str = "mars-orchestrator";
/// An address nothing listens on, so the probe never reaches a real server and
/// the `init` line reports the failure the launcher would see.
pub const UNREACHABLE_MCP_URL: &str = "http://127.0.0.1:1/mcp";

/// The credential this run launches the CLI with.
///
/// `value` is a secret: it is written into the child's environment and into
/// nothing else. `Debug` is deliberately not derived.
pub struct Credential {
    pub name: &'static str,
    pub value: String,
}

impl Credential {
    /// The obviously fake credential the `auth_failure` scenario uses in place
    /// of the real one, under the same variable name.
    pub fn fake(name: &'static str) -> Self {
        Self {
            name,
            value: FAKE_CREDENTIAL.to_string(),
        }
    }
}

/// One enabled probe run: the binary, the credential, the throwaway
/// directories and the recorder.
pub struct Probe {
    pub bin: String,
    pub credential: Credential,
    /// The host's `HOME`, which stays exactly as it is for the child and must
    /// never appear in a fixture.
    pub home: String,
    /// The CLI's working directory.
    pub work: TempDir,
    /// `CLAUDE_CONFIG_DIR`, so the probe never touches the operator's own
    /// Claude configuration.
    pub config: TempDir,
    /// Probe-owned files, such as the `--mcp-config` document; kept out of the
    /// working directory so a scenario can assert what the CLI discovers
    /// there.
    pub state: TempDir,
    pub recorder: Recorder,
}

/// The probe for this run, or `None` when it is not enabled.
///
/// Returning `None` is the skip: the caller returns and the test passes. When
/// the probe *is* enabled the environment has to be exactly right, so a
/// missing or doubled credential fails here rather than costing a launch.
pub fn probe_or_skip() -> Option<Probe> {
    if std::env::var(ENABLE_VAR).ok().as_deref() != Some("1") {
        eprintln!("{SKIPPED}");
        return None;
    }

    Some(Probe::from_env())
}

impl Probe {
    /// The probe an enabled run works with. Panics — fails the scenario — on
    /// any environment the probe must not run against.
    pub fn from_env() -> Self {
        let credential = credential_from_env();
        let home = std::env::var("HOME").expect("HOME must be set");

        // Once per process, before any scenario can record: the scenarios run
        // concurrently and each creates the version directory when it records,
        // so a per-scenario check would trip over a sibling's fresh directory.
        static RECORD_GUARD: OnceLock<Result<(), String>> = OnceLock::new();
        if let Err(message) = RECORD_GUARD.get_or_init(|| {
            let recorder = Recorder::from_env();
            guard_version_dir(&recorder.root, CLAUDE_CLI_VERSION, recorder.enabled)
        }) {
            panic!("{message}");
        }

        Self {
            bin: std::env::var(BIN_VAR).unwrap_or_else(|_| DEFAULT_BIN.to_string()),
            credential,
            home,
            work: TempDir::new().expect("a working directory"),
            config: TempDir::new().expect("a config directory"),
            state: TempDir::new().expect("a state directory"),
            recorder: Recorder::from_env(),
        }
    }

    /// Run the probe with the fake credential instead of the real one, under
    /// the same variable name (`auth_failure`).
    pub fn with_fake_credential(mut self) -> Self {
        self.credential = Credential::fake(self.credential.name);
        self
    }

    pub fn work_path(&self) -> &Path {
        self.work.path()
    }

    /// Write an `mcpServers` document with one entry and return its path, for
    /// [`LaunchContext::mcp_config_path`].
    ///
    /// The launcher's `--mcp-config` path is fixed at `/session/mcp.json`
    /// inside a container; on the host the probe points the context at this
    /// file instead, which is why the path is a context field rather than a
    /// constant in the adapter.
    pub fn write_mcp_config(&self, name: &str, url: &str) -> PathBuf {
        let path = self.state.path().join("mcp.json");
        std::fs::write(&path, mcp_servers_document(name, url)).expect("the mcp config is written");
        path
    }

    /// Write a repository-owned `.mcp.json` into the working directory, which
    /// is the configuration `--strict-mcp-config` is expected to ignore
    /// (`docs/open-questions.md`, item 4).
    pub fn write_repo_mcp_config(&self, name: &str, url: &str) {
        std::fs::write(
            self.work_path().join(".mcp.json"),
            mcp_servers_document(name, url),
        )
        .expect("the repository mcp config is written");
    }

    /// A launch context whose `--mcp-config` names a file on this host.
    pub fn context(&self, mode: LaunchMode, mcp_config: &Path) -> LaunchContext {
        let mut ctx = LaunchContext::new(mode);
        ctx.mcp_config_path = mcp_config.to_string_lossy().into_owned();
        ctx
    }

    /// Fail the run unless the CLI on `PATH` is the pinned version.
    ///
    /// The whole point of the probe is that the recorded behaviour belongs to
    /// one version, so a mismatch fails loudly with both versions named rather
    /// than producing fixtures attributed to the wrong one.
    pub async fn assert_pinned_version(&self) {
        let output = Command::new(&self.bin)
            .arg("--version")
            .output()
            .await
            .unwrap_or_else(|error| panic!("`{} --version` did not run: {error}", self.bin));

        let reported = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert!(
            reported.starts_with(CLAUDE_CLI_VERSION),
            "the CLI on PATH is not the pinned version: pinned {CLAUDE_CLI_VERSION}, found {reported}",
        );
    }
}

/// One `mcpServers` document with a single HTTP entry.
fn mcp_servers_document(name: &str, url: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "mcpServers": { name: { "type": "http", "url": url } },
    }))
    .expect("the document serialises")
}

/// The single credential in the environment.
///
/// Both set is the launcher's own error case and fails before anything is
/// spawned; neither set with the probe explicitly enabled is an operator
/// mistake worth failing on rather than silently skipping.
fn credential_from_env() -> Credential {
    let present: Vec<&'static str> = CREDENTIAL_VARS
        .into_iter()
        .filter(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
        .collect();

    match present.as_slice() {
        [name] => Credential {
            name,
            value: std::env::var(name).expect("the variable was just read"),
        },
        [] => panic!(
            "{ENABLE_VAR}=1 but neither {} nor {} is set",
            CREDENTIAL_VARS[0], CREDENTIAL_VARS[1],
        ),
        _ => panic!(
            "both {} and {} are set; a launch carries exactly one credential",
            CREDENTIAL_VARS[0], CREDENTIAL_VARS[1],
        ),
    }
}

/// One launched CLI process, its stdout split into lines and everything
/// written to its stdin remembered for the recorder.
pub struct Session {
    /// The scenario this belongs to, for failure messages and fixture names.
    pub scenario: &'static str,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: UnboundedReceiver<String>,
    /// Every stdout line, verbatim and in order.
    pub stdout: Vec<String>,
    /// Every line written to stdin, verbatim and in order.
    pub stdin_lines: Vec<String>,
    deadline: Instant,
    /// The child's exit status, once it has been waited for.
    pub exit: Option<ExitStatus>,
}

impl Session {
    /// Launch `argv` — the adapter's own, with its first element replaced by
    /// the binary this run probes — in the probe's working directory.
    ///
    /// `HOME` is inherited unchanged; only `CLAUDE_CONFIG_DIR` and the one
    /// credential variable are set, so the child sees the operator's shell
    /// minus the other credential.
    pub fn launch(probe: &Probe, scenario: &'static str, argv: &[String]) -> Self {
        let (program, args) = argv.split_first().expect("argv names the binary");
        assert_eq!(
            program,
            mars_orchestrator::agent::claude::CLAUDE_CLI_BINARY,
            "the adapter's argv should start with the CLI binary",
        );

        let other = CREDENTIAL_VARS
            .into_iter()
            .find(|name| *name != probe.credential.name)
            .expect("two credential variables");

        let mut command = Command::new(&probe.bin);
        command
            .args(args)
            .current_dir(probe.work_path())
            .env("CLAUDE_CONFIG_DIR", probe.config.path())
            .env(probe.credential.name, &probe.credential.value)
            .env_remove(other)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("{scenario}: `{}` did not start: {error}", probe.bin));

        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, rx) = unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Self {
            scenario,
            child,
            stdin: Some(stdin),
            lines: rx,
            stdout: Vec::new(),
            stdin_lines: Vec::new(),
            deadline: Instant::now() + SCENARIO_TIMEOUT,
            exit: None,
        }
    }

    /// Write one already-encoded line — [`encode_input`](mars_orchestrator::agent::AgentBackend::encode_input)
    /// produces the trailing newline — and remember it.
    pub async fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        let line = if line.ends_with('\n') {
            line.to_string()
        } else {
            format!("{line}\n")
        };

        stdin
            .write_all(line.as_bytes())
            .await
            .expect("stdin accepts the line");
        stdin.flush().await.expect("stdin flushes");
        self.stdin_lines.push(line.trim_end().to_string());
    }

    /// Close stdin, which is how a conversational process is ended.
    pub async fn close_stdin(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.shutdown().await;
        }
    }

    /// The next stdout line, or `None` at end of output.
    ///
    /// A read that outlives the scenario's budget kills the child and fails
    /// with what was seen, which is the only useful thing a timed-out probe
    /// can say.
    pub async fn next_line(&mut self) -> Option<String> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, self.lines.recv()).await {
            Ok(Some(line)) => {
                self.stdout.push(line.clone());
                Some(line)
            }
            Ok(None) => None,
            Err(_) => self.timed_out("waiting for the next line"),
        }
    }

    /// Read until a line satisfies `predicate`, and return it parsed.
    pub async fn read_until(
        &mut self,
        what: &str,
        predicate: impl Fn(&Value) -> bool,
    ) -> serde_json::Map<String, Value> {
        while let Some(line) = self.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if predicate(&value) {
                return value.as_object().cloned().unwrap_or_default();
            }
        }

        self.timed_out(&format!("output ended before {what}"))
    }

    /// Read until the CLI ends a turn.
    pub async fn read_turn(&mut self) -> serde_json::Map<String, Value> {
        self.read_until("a `result` line", is_result).await
    }

    /// Read whatever is already buffered, without waiting for more.
    pub async fn drain_available(&mut self) {
        while let Ok(line) = self.lines.try_recv() {
            self.stdout.push(line);
        }
    }

    /// Read until the process closes stdout.
    pub async fn read_to_end(&mut self) {
        while self.next_line().await.is_some() {}
    }

    /// Close stdin, read the rest of the output and wait for the exit status.
    pub async fn finish(&mut self) -> ExitStatus {
        self.close_stdin().await;
        self.read_to_end().await;

        let left = self.deadline.saturating_duration_since(Instant::now());
        let status = match tokio::time::timeout(left, self.child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => panic!(
                "{}: the child could not be waited for: {error}",
                self.scenario
            ),
            Err(_) => self.timed_out("waiting for the process to exit"),
        };

        self.exit = Some(status);
        status
    }

    /// Every stdout line parsed, with unparsable lines skipped.
    pub fn json_lines(&self) -> Vec<Value> {
        self.stdout
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
    }

    /// The `type` of every line seen so far, in order.
    pub fn line_types(&self) -> Vec<String> {
        self.json_lines()
            .iter()
            .map(|value| string_at(value, "type").unwrap_or_else(|| "?".to_string()))
            .collect()
    }

    /// The first line satisfying `predicate`, parsed.
    pub fn find(&self, predicate: impl Fn(&Value) -> bool) -> Option<Value> {
        self.json_lines().into_iter().find(|value| predicate(value))
    }

    /// Every line satisfying `predicate`, parsed.
    pub fn filter(&self, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
        self.json_lines()
            .into_iter()
            .filter(|value| predicate(value))
            .collect()
    }

    fn timed_out(&mut self, what: &str) -> ! {
        let _ = self.child.start_kill();
        eprintln!(
            "{}: {} lines seen before the timeout:",
            self.scenario,
            self.stdout.len()
        );
        for line in &self.stdout {
            eprintln!("  {line}");
        }
        panic!(
            "{}: timed out after {}s {what}",
            self.scenario,
            SCENARIO_TIMEOUT.as_secs(),
        );
    }
}

/// Whether a native line is the `system`/`init` one.
pub fn is_init(value: &Value) -> bool {
    string_at(value, "type").as_deref() == Some("system")
        && string_at(value, "subtype").as_deref() == Some("init")
}

/// Whether a native line ends a turn.
pub fn is_result(value: &Value) -> bool {
    string_at(value, "type").as_deref() == Some("result")
}

/// Whether a native line is an assistant message.
pub fn is_assistant(value: &Value) -> bool {
    string_at(value, "type").as_deref() == Some("assistant")
}

/// One string field of a native line, when it is a string.
pub fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// All assistant text of a turn, concatenated, so a scenario can assert what
/// the model answered without walking the block structure itself.
pub fn assistant_text(lines: &[Value]) -> String {
    let mut text = String::new();
    for line in lines.iter().filter(|line| is_assistant(line)) {
        let Some(blocks) = line
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block in blocks {
            if string_at(block, "type").as_deref() == Some("text")
                && let Some(block_text) = string_at(block, "text")
            {
                text.push_str(&block_text);
                text.push('\n');
            }
        }
    }
    text
}

/// The fixture recorder (`CLAUDE.md`, "Testing expectations": one directory
/// per pinned CLI version, a bump adds fixtures and never edits old ones).
pub struct Recorder {
    root: PathBuf,
    enabled: bool,
}

impl Recorder {
    /// The recorder for this run, rooted at `tests/fixtures/claude`.
    pub fn from_env() -> Self {
        Self {
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("claude"),
            enabled: std::env::var(RECORD_VAR).ok().as_deref() == Some("1"),
        }
    }

    /// A recorder writing under `root`, for the guard's own tests.
    pub fn rooted(root: PathBuf, enabled: bool) -> Self {
        Self { root, enabled }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The directory this version's fixtures go in.
    pub fn version_dir(&self) -> PathBuf {
        self.root.join(CLAUDE_CLI_VERSION)
    }

    /// Fail before any scenario runs if recording would overwrite a version
    /// that is already on disk.
    pub fn assert_can_record(&self) {
        if let Err(message) = guard_version_dir(&self.root, CLAUDE_CLI_VERSION, self.enabled) {
            panic!("{message}");
        }
    }

    /// Scan a finished session and, when recording is on, write its two
    /// fixture files and append `notes` to `NOTES.md`.
    ///
    /// The scan runs whether or not recording is on: a leak is a finding about
    /// the CLI's output, not about the recorder, and it should fail the
    /// scenario either way.
    pub fn record(&self, probe: &Probe, session: &Session, notes: &str) {
        if let Err(message) = scan_lines(&session.stdout, &probe.credential.value, &probe.home) {
            panic!("{}: stdout {message}", session.scenario);
        }
        if let Err(message) = scan_lines(&session.stdin_lines, &probe.credential.value, &probe.home)
        {
            panic!("{}: stdin {message}", session.scenario);
        }

        if !self.enabled {
            return;
        }

        let dir = self.version_dir();
        std::fs::create_dir_all(&dir).expect("the version directory is created");
        write_lines(
            &dir.join(format!("{}.jsonl", session.scenario)),
            &session.stdout,
        );
        write_lines(
            &dir.join(format!("{}.stdin.jsonl", session.scenario)),
            &session.stdin_lines,
        );
        append_notes(&dir.join("NOTES.md"), session.scenario, notes);
    }
}

/// The no-overwrite rule: recording into a version directory that already
/// exists is refused, so a recorded version is immutable.
pub fn guard_version_dir(root: &Path, version: &str, recording: bool) -> Result<(), String> {
    if recording && root.join(version).exists() {
        return Err(format!(
            "fixtures for {version} already recorded; bump CLAUDE_CLI_VERSION or delete locally",
        ));
    }

    Ok(())
}

fn write_lines(path: &Path, lines: &[String]) {
    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    std::fs::write(path, body).expect("the fixture is written");
}

/// Append one scenario's observations to the human-readable notes.
///
/// Markdown, one heading per scenario, observations only: no raw payloads, so
/// the file stays something the follow-up documentation task can read.
fn append_notes(path: &Path, scenario: &str, notes: &str) {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the notes file opens");

    if file.metadata().expect("the notes metadata").len() == 0 {
        writeln!(
            file,
            "# Claude Code {CLAUDE_CLI_VERSION} — live probe observations\n\n\
             Recorded by `tests/claude_probe.rs`. Observations only; the native \
             lines are in the `.jsonl` files beside this one.\n"
        )
        .expect("the notes header is written");
    }

    writeln!(file, "## {scenario}\n\n{}\n", notes.trim_end()).expect("the notes are written");
}

/// Refuse any line that would put a secret, or the operator's home path, into
/// a fixture (rule 3).
///
/// The failure names what was found and where, never the value itself.
pub fn scan_lines(lines: &[String], credential: &str, home: &str) -> Result<(), String> {
    for (index, line) in lines.iter().enumerate() {
        scan_line(line, credential, home)
            .map_err(|what| format!("line {} contains {what}", index + 1))?;
    }

    Ok(())
}

/// One line's scan. `Err` carries what was found, for [`scan_lines`] to place.
pub fn scan_line(line: &str, credential: &str, home: &str) -> Result<(), String> {
    if credential != FAKE_CREDENTIAL && !credential.is_empty() && line.contains(credential) {
        return Err("the credential value".to_string());
    }

    if !home.is_empty() && home != "/" && line.contains(home) {
        return Err("the host home path".to_string());
    }

    let mut rest = line;
    while let Some(start) = rest.find(KEY_PREFIX) {
        let token = &rest[start..];
        let end = token
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(token.len());
        if &token[..end] != FAKE_CREDENTIAL {
            return Err(format!("an {KEY_PREFIX} token"));
        }
        rest = &token[end..];
    }

    Ok(())
}
