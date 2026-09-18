//! The fixture-based translation suite for the Claude backend (ADR 0008;
//! `CLAUDE.md`, "Testing expectations").
//!
//! Every directory under `tests/fixtures/claude/<version>/` is one pinned CLI
//! version. Every `<scenario>.jsonl` in it is a recording (or, prefixed
//! `synthetic_`, a hand-written line set for a rule the live CLI cannot be made
//! to produce on demand), and every recording must have a `<scenario>.expected.json`
//! stating the `TranslateState` the owner would have built and the exact
//! `AgentEvent` sequence the translator must answer with.
//!
//! The runner discovers directories rather than listing them, so a CLI version
//! bump is a new directory and no edit here. The layout, the generator marker
//! and the never-edit rule are documented in `tests/fixtures/claude/README.md`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use mars_orchestrator::agent::{
    AgentBackend, CLAUDE_CLI_VERSION, ClaudeBackend, CredentialName, InjectedCredential,
    TranslateConfig, TranslateState,
};
use mars_orchestrator::models::SecretScope;
use serde_json::Value;

/// Where the per-version directories live.
fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude")
}

/// One recording and the files that belong to it.
#[derive(Debug, Clone)]
struct Scenario {
    version: String,
    name: String,
    native: PathBuf,
    stdin: PathBuf,
    expected: PathBuf,
}

impl Scenario {
    fn label(&self) -> String {
        format!("{}/{}", self.version, self.name)
    }
}

/// Every version directory under `tests/fixtures/claude/`, sorted.
fn version_dirs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs = Vec::new();
    let entries =
        fs::read_dir(root).map_err(|e| format!("could not read {}: {e}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("could not read {}: {e}", root.display()))?;
        if entry.path().is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Every scenario in one version directory.
///
/// A `<scenario>.jsonl` without a `<scenario>.expected.json` is an error, not a
/// skip: a recording that nobody wrote expectations for would otherwise sit in
/// the tree proving nothing.
fn scenarios_in(dir: &Path) -> Result<Vec<Scenario>, String> {
    let version = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no directory name", dir.display()))?
        .to_string();

    let mut names = BTreeSet::new();
    let entries =
        fs::read_dir(dir).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("could not read {}: {e}", dir.display()))?;
        let Some(file) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // `<scenario>.stdin.jsonl` belongs to its scenario and is not one.
        let Some(name) = file.strip_suffix(".jsonl") else {
            continue;
        };
        if name.ends_with(".stdin") {
            continue;
        }
        names.insert(name.to_string());
    }

    let mut scenarios = Vec::new();
    for name in names {
        let expected = dir.join(format!("{name}.expected.json"));
        if !expected.is_file() {
            return Err(format!("missing expected events for {version}/{name}"));
        }
        scenarios.push(Scenario {
            version: version.clone(),
            name: name.clone(),
            native: dir.join(format!("{name}.jsonl")),
            stdin: dir.join(format!("{name}.stdin.jsonl")),
            expected,
        });
    }
    Ok(scenarios)
}

/// The `state` half of an expected file.
fn state_from(value: &Value, label: &str) -> Result<TranslateState, String> {
    let state = value
        .get("state")
        .ok_or_else(|| format!("{label}: the expected file has no `state`"))?;

    let flag = |key: &str| -> Result<bool, String> {
        match state.get(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(other) => Err(format!("{label}: `state.{key}` is not a boolean: {other}")),
        }
    };

    let credential = match state.get("credential") {
        None | Some(Value::Null) => None,
        Some(credential) => {
            let name = match credential.get("name").and_then(Value::as_str) {
                Some("ANTHROPIC_API_KEY") => CredentialName::AnthropicApiKey,
                Some("CLAUDE_CODE_OAUTH_TOKEN") => CredentialName::ClaudeCodeOauthToken,
                other => {
                    return Err(format!(
                        "{label}: `state.credential.name` is not a credential variable: {other:?}"
                    ));
                }
            };
            let scope = match credential.get("scope").and_then(Value::as_str) {
                Some("global") => SecretScope::Global,
                Some("user") => SecretScope::User,
                Some("project") => SecretScope::Project,
                other => {
                    return Err(format!(
                        "{label}: `state.credential.scope` is not a secret scope: {other:?}"
                    ));
                }
            };
            Some(InjectedCredential { name, scope })
        }
    };

    Ok(TranslateState::new(TranslateConfig {
        resumed: flag("resumed")?,
        partial_messages: flag("partial_messages")?,
        credential,
    }))
}

/// The `events` half of an expected file, as raw JSON.
fn expected_events(value: &Value, label: &str) -> Result<Vec<Value>, String> {
    match value.get("events") {
        Some(Value::Array(events)) => Ok(events.clone()),
        _ => Err(format!("{label}: the expected file has no `events` array")),
    }
}

/// Expand every `{"__generate": ..., "n": <bytes>}` marker into the filler
/// string it stands for, in place.
///
/// Both a native line and an expected event go through this, so a 300 KiB tool
/// result is a one-line fixture and a one-line expectation rather than 300 KiB
/// of committed text (`tests/fixtures/claude/README.md`).
fn expand_generators(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.contains_key("__generate") {
                let n = map.get("n").and_then(Value::as_u64).unwrap_or(0) as usize;
                // One ASCII byte, so the expanded length in bytes is exactly
                // `n` and the cut can never land inside a character.
                let byte = map
                    .get("fill")
                    .and_then(Value::as_str)
                    .and_then(|fill| fill.as_bytes().first().copied())
                    .filter(u8::is_ascii)
                    .unwrap_or(b'x');
                let text = String::from_utf8(vec![byte; n]).expect("ascii filler");
                *value = Value::String(text);
                return;
            }
            for field in map.values_mut() {
                expand_generators(field);
            }
        }
        Value::Array(items) => {
            for item in items {
                expand_generators(item);
            }
        }
        _ => {}
    }
}

/// One native line as the translator should see it: verbatim, unless it carries
/// a generator marker.
fn native_line(line: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(line) else {
        return line.to_string();
    };
    let before = value.clone();
    expand_generators(&mut value);
    if value == before {
        line.to_string()
    } else {
        value.to_string()
    }
}

/// The message texts a `.stdin.jsonl` would have written into the process.
///
/// The raw text of the message, which is what `record_sent_input` stores: a
/// `control_request` line writes no message and seeds nothing.
fn sent_inputs(path: &Path) -> Result<Vec<String>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let body =
        fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;

    let mut texts = Vec::new();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("user") {
            continue;
        }
        match value.pointer("/message/content") {
            Some(Value::String(text)) => texts.push(text.clone()),
            Some(Value::Array(blocks)) => {
                let mut text = String::new();
                for block in blocks {
                    match block.get("text").and_then(Value::as_str) {
                        Some(part) => text.push_str(part),
                        None => {
                            return Err(format!("{}: a stdin block has no text", path.display()));
                        }
                    }
                }
                texts.push(text);
            }
            _ => {}
        }
    }
    Ok(texts)
}

/// The expected file, with every generator marker expanded.
fn expected_body(scenario: &Scenario) -> Result<Value, String> {
    let body = fs::read_to_string(&scenario.expected)
        .map_err(|e| format!("could not read {}: {e}", scenario.expected.display()))?;
    let mut body: Value = serde_json::from_str(&body)
        .map_err(|e| format!("{}: the expected file is not JSON: {e}", scenario.label(),))?;
    expand_generators(&mut body);
    Ok(body)
}

/// Run one scenario's native lines through the translator, in order.
///
/// The state comes from the expected file and the sent-input hashes from the
/// `.stdin.jsonl`; each line's events are appended as a group, so the runner
/// never reorders.
fn translate_scenario(scenario: &Scenario) -> Result<Vec<Value>, String> {
    let label = scenario.label();
    let body = expected_body(scenario)?;
    let mut state = state_from(&body, &label)?;

    for text in sent_inputs(&scenario.stdin)? {
        state.record_sent_input(&text);
    }

    let backend = ClaudeBackend::new();
    let native = fs::read_to_string(&scenario.native)
        .map_err(|e| format!("could not read {}: {e}", scenario.native.display()))?;

    let mut actual = Vec::new();
    for line in native.lines() {
        for event in backend.translate(&native_line(line), &mut state) {
            let event = serde_json::to_value(&event)
                .map_err(|e| format!("{label}: an event did not serialise: {e}"))?;
            actual.push(event);
        }
    }
    Ok(actual)
}

/// Translate one scenario and compare, event by event, against its expected
/// file.
fn check_scenario(scenario: &Scenario) -> Result<Vec<Value>, String> {
    let label = scenario.label();
    let expected = expected_events(&expected_body(scenario)?, &label)?;
    let actual = translate_scenario(scenario)?;

    for (index, expected_event) in expected.iter().enumerate() {
        match actual.get(index) {
            Some(actual_event) if actual_event == expected_event => {}
            Some(actual_event) => {
                return Err(format!(
                    "{label}: first differing index {index}\n  expected: {expected_event}\n  actual:   {actual_event}"
                ));
            }
            None => {
                return Err(format!(
                    "{label}: first differing index {index}\n  expected: {expected_event}\n  actual:   <no event: the translator produced {} of {} events>",
                    actual.len(),
                    expected.len()
                ));
            }
        }
    }

    if actual.len() > expected.len() {
        let index = expected.len();
        return Err(format!(
            "{label}: first differing index {index}\n  expected: <no event: the file lists {} events>\n  actual:   {}",
            expected.len(),
            actual[index]
        ));
    }

    Ok(actual)
}

/// Every scenario of every version, or the first problem found.
fn all_scenarios() -> Result<Vec<Scenario>, String> {
    let root = fixtures_root();
    let mut scenarios = Vec::new();
    for dir in version_dirs(&root)? {
        scenarios.extend(scenarios_in(&dir)?);
    }
    Ok(scenarios)
}

/// The review aid: `MARS_WRITE_EXPECTED=1` rewrites each expected file's
/// `events` with what the translator answers, keeping its `state`, and then
/// fails so the run can never be mistaken for a passing one.
///
/// Its output is a starting point to read line by line against the recording
/// and `SPEC.md`, never a expectation in itself — an expected file generated
/// and committed unreviewed would only prove the translator agrees with itself
/// (`tests/fixtures/claude/README.md`).
fn write_expected(scenario: &Scenario) -> Result<(), String> {
    let label = scenario.label();
    let body = fs::read_to_string(&scenario.expected)
        .map_err(|e| format!("could not read {}: {e}", scenario.expected.display()))?;
    let mut body: Value =
        serde_json::from_str(&body).map_err(|e| format!("{label}: not JSON: {e}"))?;

    body["events"] = Value::Array(translate_scenario(scenario)?);
    fs::write(
        &scenario.expected,
        serde_json::to_string_pretty(&body).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())
}

#[test]
fn every_recorded_scenario_translates_to_its_expected_events() {
    let scenarios = all_scenarios().expect("the fixture tree is complete");
    assert!(
        !scenarios.is_empty(),
        "no scenarios under {}",
        fixtures_root().display()
    );

    if std::env::var_os("MARS_WRITE_EXPECTED").is_some() {
        for scenario in &scenarios {
            write_expected(scenario).expect("an expected file could be rewritten");
        }
        panic!(
            "MARS_WRITE_EXPECTED rewrote every expected file; review each one against its recording before committing"
        );
    }

    let mut failures = Vec::new();
    for scenario in &scenarios {
        if let Err(failure) = check_scenario(scenario) {
            failures.push(failure);
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

#[test]
fn the_pinned_cli_version_has_a_fixture_directory() {
    let dir = fixtures_root().join(CLAUDE_CLI_VERSION);
    assert!(
        dir.is_dir(),
        "no fixtures for the pinned CLI version at {}",
        dir.display()
    );
}

/// The scenarios the probe recorded, plus the synthetic ones, all of which the
/// pinned version must carry.
const REQUIRED_PINNED_SCENARIOS: [&str; 19] = [
    "auth_failure",
    "ephemeral",
    "mcp_config",
    "mcp_config_strict",
    "mid_turn",
    "multi_turn",
    "prompt_kind",
    "resume_prompt",
    "resume_prompt_first",
    "stdin_shape",
    "subagent",
    "synthetic_auth_no_credential",
    "synthetic_denials",
    "synthetic_echo",
    "synthetic_init_resumed",
    "synthetic_partial",
    "synthetic_raw",
    "synthetic_thinking",
    "synthetic_truncation",
];

#[test]
fn the_pinned_version_carries_every_required_scenario() {
    let scenarios =
        scenarios_in(&fixtures_root().join(CLAUDE_CLI_VERSION)).expect("the fixture tree");
    let names: BTreeSet<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
    for required in REQUIRED_PINNED_SCENARIOS {
        assert!(
            names.contains(required),
            "the pinned version has no `{required}` scenario"
        );
    }
}

#[test]
fn the_subagent_recording_proves_the_subagent_tool_name() {
    let scenarios =
        scenarios_in(&fixtures_root().join(CLAUDE_CLI_VERSION)).expect("the fixture tree");
    let subagent = scenarios
        .iter()
        .find(|s| s.name == "subagent")
        .expect("the subagent scenario");
    let events = check_scenario(subagent).expect("the subagent scenario translates");

    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|event| event.get("kind").and_then(Value::as_str))
        .collect();
    assert!(
        kinds.contains(&"subagent_start"),
        "the subagent recording produced no subagent_start; SUBAGENT_TOOL_NAMES no longer matches {CLAUDE_CLI_VERSION}"
    );
    assert!(
        kinds.contains(&"subagent_end"),
        "the subagent recording produced no subagent_end"
    );
}

/// Every `AgentEventBody` kind the Claude translator can emit.
///
/// `user_message`, `state_change`, `launch_warning` and `git` are not
/// here: the owner writes them, never the translator.
const TRANSLATOR_EVENT_KINDS: [&str; 12] = [
    "error",
    "init",
    "permission_denied",
    "raw",
    "result",
    "subagent_end",
    "subagent_start",
    "text",
    "text_delta",
    "thinking",
    "tool_call",
    "tool_result",
];

#[test]
fn every_translator_event_kind_is_covered_by_the_pinned_version() {
    let scenarios =
        scenarios_in(&fixtures_root().join(CLAUDE_CLI_VERSION)).expect("the fixture tree");

    let mut seen = BTreeSet::new();
    for scenario in &scenarios {
        let body = fs::read_to_string(&scenario.expected).expect("an expected file");
        let body: Value = serde_json::from_str(&body).expect("an expected file is JSON");
        let events = expected_events(&body, &scenario.label()).expect("an events array");
        for event in events {
            if let Some(kind) = event.get("kind").and_then(Value::as_str) {
                seen.insert(kind.to_string());
            }
        }
    }

    for kind in TRANSLATOR_EVENT_KINDS {
        assert!(
            seen.contains(kind),
            "no expected file for {CLAUDE_CLI_VERSION} asserts a `{kind}` event, so that rule is uncovered"
        );
    }
}

#[test]
fn no_fixture_carries_a_credential() {
    // The one API-key-shaped string the probe is allowed to have recorded, and
    // the one placeholder a header may carry (rule 3).
    const ALLOWED_KEY: &str = "sk-ant-fake-probe-0000";
    const ALLOWED_BEARER: &str = "Bearer <token>";

    let root = fixtures_root();
    let mut offences = Vec::new();
    for dir in version_dirs(&root).expect("the fixture tree") {
        for entry in fs::read_dir(&dir).expect("a version directory") {
            let path = entry.expect("a fixture file").path();
            if !path.is_file() {
                continue;
            }
            let Ok(body) = fs::read_to_string(&path) else {
                continue;
            };
            for (number, line) in body.lines().enumerate() {
                let mut rest = line;
                while let Some(at) = rest.find("sk-ant-") {
                    let tail = &rest[at..];
                    if !tail.starts_with(ALLOWED_KEY) {
                        offences.push(format!("{}:{}: sk-ant- key", path.display(), number + 1));
                    }
                    rest = &tail[ALLOWED_KEY.len().min(tail.len())..];
                }
                let mut rest = line;
                while let Some(at) = rest.find("Bearer ") {
                    let tail = &rest[at..];
                    if !tail.starts_with(ALLOWED_BEARER) {
                        offences.push(format!("{}:{}: bearer token", path.display(), number + 1));
                    }
                    rest = &tail["Bearer ".len()..];
                }
            }
        }
    }
    assert!(offences.is_empty(), "\n{}", offences.join("\n"));
}

/// A scenario file written into a temp directory, so the runner's own failure
/// paths are exercised without a wrong file ever living under `fixtures/`.
fn temp_scenario(native: &str, expected: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let version = dir.path().join("9.9.9");
    fs::create_dir(&version).expect("a version dir");
    fs::write(version.join("probe.jsonl"), native).expect("a native file");
    if let Some(expected) = expected {
        fs::write(version.join("probe.expected.json"), expected).expect("an expected file");
    }
    (dir, version)
}

#[test]
fn a_recording_without_expected_events_fails_the_runner() {
    let (_guard, version) = temp_scenario("{\"type\":\"rate_limit_event\"}\n", None);
    let error = scenarios_in(&version).expect_err("a recording with no expectations is an error");
    assert_eq!(error, "missing expected events for 9.9.9/probe");
}

#[test]
fn a_wrong_expected_file_names_the_first_differing_index() {
    let (_guard, version) = temp_scenario(
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"one\"},{\"type\":\"text\",\"text\":\"two\"}]}}\n",
        Some(
            r#"{"state":{"resumed":false,"partial_messages":false,"credential":null},
                "events":[{"kind":"text","text":"one"},{"kind":"text","text":"WRONG"}]}"#,
        ),
    );
    let scenarios = scenarios_in(&version).expect("one scenario");
    let error = check_scenario(&scenarios[0]).expect_err("a wrong expected file fails");
    assert!(
        error.starts_with("9.9.9/probe: first differing index 1"),
        "{error}"
    );
    assert!(error.contains("\"WRONG\""), "{error}");
    assert!(error.contains("\"two\""), "{error}");
}

#[test]
fn an_expected_file_with_too_few_events_names_the_first_extra_one() {
    let (_guard, version) = temp_scenario(
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"one\"},{\"type\":\"text\",\"text\":\"two\"}]}}\n",
        Some(r#"{"state":{},"events":[{"kind":"text","text":"one"}]}"#),
    );
    let scenarios = scenarios_in(&version).expect("one scenario");
    let error = check_scenario(&scenarios[0]).expect_err("a short expected file fails");
    assert!(
        error.starts_with("9.9.9/probe: first differing index 1"),
        "{error}"
    );
    assert!(error.contains("\"two\""), "{error}");
}

#[test]
fn the_generator_marker_expands_to_the_byte_count_it_names() {
    let mut value: Value =
        serde_json::from_str(r#"{"content":{"__generate":"tool_result_bytes","n":1024}}"#)
            .expect("json");
    expand_generators(&mut value);
    let text = value["content"].as_str().expect("an expanded string");
    assert_eq!(text.len(), 1024);
    assert!(text.chars().all(|c| c == 'x'));
}
