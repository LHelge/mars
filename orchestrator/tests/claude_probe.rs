//! The live probe against the pinned Claude Code CLI (`ARCHITECTURE.md`,
//! "Input encoding": "the first integration test of the Claude adapter is
//! therefore a live probe against the pinned CLI version").
//!
//! It answers the verification items in `docs/open-questions.md` by
//! observation rather than by reading the CLI reference, and records the
//! native output as the fixtures every later translation test is built from.
//! Each scenario is named after the item it answers:
//!
//! | Scenario | Item |
//! | --- | --- |
//! | `stdin_shape` | 1, the stdin user-message shape |
//! | `mid_turn` | 2, mid-turn writes and `control_request`/`interrupt` |
//! | `prompt_kind` | 3, whether anything can prompt the host |
//! | `mcp_config` | 4 and 6, `--strict-mcp-config` and the `init` fields |
//! | `multi_turn` | 5 and 7, cumulative or per-turn cost, multi-turn stdin |
//! | `resume_prompt` | 7, a changed system prompt on `--resume` (ADR 0003) |
//! | `subagent` | 7, `--forward-subagent-text` and `parent_tool_use_id` |
//! | `ephemeral` | the `-p` shape |
//! | `auth_failure` | the shape the translator's authentication rule matches |
//!
//! The task that commissioned this probe numbered the multi-turn, resume and
//! subagent verification as item 9; the document has carried them as item 7
//! since the list was renumbered, and that is the numbering used here.
//!
//! **It is opt-in and costs money.** Without `MARS_CLAUDE_PROBE=1` every
//! scenario prints one line and passes, which is how CI and every ordinary
//! `cargo test` run see it. Enabled, it needs exactly one of
//! `ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN` — the launcher's own rule
//! — and the `claude` on `PATH` must be the pinned version
//! (`MARS_CLAUDE_BIN` overrides the path). The prompts are deliberately tiny:
//! the whole probe should cost well under one US dollar.
//!
//! **What it asserts and what it only records.** It asserts what the code
//! already claims: that the adapter's own argv launches, that the bare stdin
//! line is accepted, that a resumed process honours the new system prompt,
//! that a subagent's frames carry `parent_tool_use_id` and that every recorded
//! line survives `ClaudeBackend::translate`. Everything the documents do not
//! yet decide — queued or interrupted, cumulative or per-turn cost, which
//! fields `init` carries — is written into `NOTES.md` for the follow-up
//! documentation task to read; deciding it here would be inventing an answer.
//!
//! Recording is the second opt-in, `MARS_RECORD_FIXTURES=1`, and never
//! overwrites an existing version directory (`CLAUDE.md`, "Testing
//! expectations").

mod common;

use std::time::Duration;

use common::claude_probe::{
    MCP_SERVER_NAME, Probe, Recorder, Session, UNREACHABLE_MCP_URL, assistant_text,
    guard_version_dir, is_assistant, is_init, is_result, probe_or_skip, scan_line, string_at,
};
use mars_orchestrator::agent::claude::{SUBAGENT_TOOL_NAMES, build_argv};
use mars_orchestrator::agent::{
    AgentBackend, ClaudeBackend, LaunchMode, TranslateConfig, TranslateState,
};
use mars_orchestrator::events::SessionInput;
use serde_json::Value;

/// The flag item 4 asks about. The adapter never emits it; the probe appends
/// it to the adapter's argv for the one comparison run.
const STRICT_MCP_CONFIG: &str = "--strict-mcp-config";

/// The repository-owned MCP server name item 4 is about.
const REPO_MCP_SERVER_NAME: &str = "repo-local";

/// A second unreachable address, so a discovered server is distinguishable
/// from the launcher's own by name alone.
const REPO_MCP_URL: &str = "http://127.0.0.1:2/mcp";

/// One turn's worth of work that takes long enough to write into.
const SLOW_PROMPT: &str = "Count slowly from 1 to 30, one number per line.";

/// The smallest possible turn.
const PING: &str = "Reply with exactly the word PONG.";

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// Item 1: the stdin user-message shape.
///
/// The bare shape is tried first. If the CLI rejects it the scenario retries
/// with `session_id` added and then fails, because the answer belongs in
/// `agent/claude/input.rs` and in `ARCHITECTURE.md`, "Input encoding" — never
/// in a silent retry that leaves production writing the rejected line.
#[tokio::test]
async fn stdin_shape() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;
    probe.recorder.assert_can_record();

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);
    let mut session = Session::launch(&probe, "stdin_shape", &build_argv(&ctx));

    let init = session.read_until("the `init` line", is_init).await;
    let cli_session_id = init
        .get("session_id")
        .and_then(Value::as_str)
        .expect("init names the session")
        .to_string();

    session
        .send(&encode(&SessionInput::Message {
            text: PING.to_string(),
        }))
        .await;
    let result = session.read_turn().await;
    let accepted = !session.filter(is_assistant).is_empty();

    let mut notes = format!(
        "- The bare `{{\"type\":\"user\",\"message\":{{...}}}}` line was {}.\n\
         - `init`, `assistant` and `result` all arrived within the scenario budget.\n\
         - `result.subtype`: {:?}, `is_error`: {:?}.\n",
        if accepted { "accepted" } else { "rejected" },
        result.get("subtype"),
        result.get("is_error"),
    );

    let mut with_session_id_worked = false;
    if !accepted {
        // The bare shape failed: find out whether `session_id` is what is
        // missing, so the failure message says what to change.
        session.finish().await;
        let mut retry = Session::launch(&probe, "stdin_shape", &build_argv(&ctx));
        retry.read_until("the `init` line", is_init).await;
        retry
            .send(&format!(
                r#"{{"type":"user","session_id":"{cli_session_id}","message":{{"role":"user","content":[{{"type":"text","text":"{PING}"}}]}}}}"#
            ))
            .await;
        retry.read_turn().await;
        with_session_id_worked = !retry.filter(is_assistant).is_empty();
        retry.finish().await;
        notes.push_str(&format!(
            "- Retried with `session_id` on the line: {}.\n",
            if with_session_id_worked {
                "accepted"
            } else {
                "also rejected"
            },
        ));
    }

    let exit = if accepted {
        Some(session.finish().await)
    } else {
        None
    };
    if let Some(exit) = exit {
        notes.push_str(&format!("- Exit status on stdin EOF: {exit}.\n"));
    }

    probe.recorder.record(&probe, &session, &notes);
    translate_recorded(&session, TranslateConfig::default());

    assert!(
        accepted,
        "the CLI rejected the documented bare stdin line; with `session_id` it was {}: \
         fix the shape in `agent/claude/input.rs` and `ARCHITECTURE.md`, \"Input encoding\"",
        if with_session_id_worked {
            "accepted"
        } else {
            "also rejected"
        },
    );
}

/// Items 5 and 7: two sequential turns, their `num_turns`, and whether
/// `total_cost_usd` accumulates across the process.
#[tokio::test]
async fn multi_turn() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);
    let mut session = Session::launch(&probe, "multi_turn", &build_argv(&ctx));
    session.read_until("the `init` line", is_init).await;

    session.send(&message(PING)).await;
    let first = session.read_turn().await;
    let after_first = session.json_lines().len();

    session
        .send(&message("Reply with exactly the word ZEBRA."))
        .await;
    let second = session.read_turn().await;
    let second_text = assistant_text(&session.json_lines()[after_first..]);
    let exit = session.finish().await;

    let (first_cost, second_cost) = (cost(&first), cost(&second));
    let notes = format!(
        "- Two sequential messages produced two `result` lines on one process.\n\
         - `num_turns`: {:?} then {:?}.\n\
         - `total_cost_usd`: {first_cost:?} then {second_cost:?} — {}.\n\
         - The second turn answered the second message: {}.\n\
         - Exit status on stdin EOF: {exit}.\n",
        first.get("num_turns"),
        second.get("num_turns"),
        match (first_cost, second_cost) {
            (Some(a), Some(b)) if b > a => "cumulative for the process",
            (Some(_), Some(_)) => "per turn (the second is not larger)",
            _ => "not reported",
        },
        second_text.contains("ZEBRA"),
    );

    probe.recorder.record(&probe, &session, &notes);
    translate_recorded(&session, TranslateConfig::default());

    assert!(
        session.filter(is_result).len() >= 2,
        "two messages should produce two `result` lines",
    );
    assert!(
        second_text.contains("ZEBRA"),
        "the second turn should answer the second message",
    );
}

/// Item 2: a message written mid-turn, and `control_request`/`interrupt`.
///
/// Nothing here is asserted: both outcomes are legitimate CLI behaviour and
/// the point is to learn which one it is. The session owner's queueing rule is
/// written from this note.
#[tokio::test]
async fn mid_turn() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);
    let mut session = Session::launch(&probe, "mid_turn", &build_argv(&ctx));
    session.read_until("the `init` line", is_init).await;

    session.send(&message(SLOW_PROMPT)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    session
        .send(&message("Reply with exactly the word ZEBRA."))
        .await;

    let first = session.read_turn().await;
    let counted_to_thirty = assistant_text(&session.json_lines()).contains("30");
    let second = session.read_turn().await;
    let queued = session.filter(is_result).len() >= 2;

    // A third long turn, interrupted with the control request item 2 asks
    // about. An unsupported request is expected to be ignored, so the read
    // that follows also accepts the turn simply finishing.
    session.send(&message(SLOW_PROMPT)).await;
    session
        .read_until("the first assistant line", is_assistant)
        .await;
    session
        .send(r#"{"type":"control_request","request_id":"probe-1","request":{"subtype":"interrupt"}}"#)
        .await;
    let ended = session
        .read_until("a `control_response` or the turn's end", |value| {
            string_at(value, "type").as_deref() == Some("control_response") || is_result(value)
        })
        .await;
    let control_response = ended.get("type").and_then(Value::as_str) == Some("control_response");
    let exit = session.finish().await;

    let notes = format!(
        "- A second message written 1 s into a turn was {}.\n\
         - The first turn {} its count to 30 before the second turn began.\n\
         - First `result`: subtype {:?}, `num_turns` {:?}; second `result`: subtype {:?}.\n\
         - `control_request`/`interrupt` produced a `control_response` line: {control_response}.\n\
         - The interrupted turn ended with: {:?}.\n\
         - Exit status on stdin EOF: {exit}.\n",
        if queued {
            "queued"
        } else {
            "not answered separately"
        },
        if counted_to_thirty {
            "finished"
        } else {
            "did not finish"
        },
        first.get("subtype"),
        first.get("num_turns"),
        second.get("subtype"),
        ended.get("subtype"),
    );

    probe.recorder.record(&probe, &session, &notes);
    translate_recorded(&session, TranslateConfig::default());
}

/// Item 3: whether anything can ask the host a question under
/// `--permission-mode bypassPermissions --permission-prompts none`.
///
/// If nothing ever does, the `prompt` event kind and the `answer` input leave
/// `SPEC.md` — which is the follow-up documentation task's decision, made from
/// this note.
#[tokio::test]
async fn prompt_kind() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);
    let mut session = Session::launch(&probe, "prompt_kind", &build_argv(&ctx));
    session.read_until("the `init` line", is_init).await;

    session
        .send(&message(
            "Use the AskUserQuestion tool to ask me which colour I prefer, then stop.",
        ))
        .await;
    let result = session.read_turn().await;
    let exit = session.finish().await;

    let asked = session.filter(|value| {
        serde_json::to_string(value)
            .unwrap_or_default()
            .contains("AskUserQuestion")
    });
    let notes = format!(
        "- Line types seen: {:?}.\n\
         - Lines mentioning `AskUserQuestion`: {}.\n\
         - The turn ended without an answer being written to stdin: subtype {:?}, \
         `is_error` {:?}.\n\
         - Exit status on stdin EOF: {exit}.\n\
         - Nothing blocked on stdin: the `result` arrived while stdin was idle.\n",
        session.line_types(),
        asked.len(),
        result.get("subtype"),
        result.get("is_error"),
    );

    probe.recorder.record(&probe, &session, &notes);
    translate_recorded(&session, TranslateConfig::default());
}

/// Items 4 and 6: what `--strict-mcp-config` does to a repository-owned
/// `.mcp.json`, and every field `init` actually carries.
#[tokio::test]
async fn mcp_config() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;

    probe.write_repo_mcp_config(REPO_MCP_SERVER_NAME, REPO_MCP_URL);
    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);

    let mut lenient = Session::launch(&probe, "mcp_config", &build_argv(&ctx));
    let lenient_init = lenient.read_until("the `init` line", is_init).await;
    lenient.send(&message(PING)).await;
    lenient.read_turn().await;
    let lenient_exit = lenient.finish().await;

    let mut argv = build_argv(&ctx);
    argv.push(STRICT_MCP_CONFIG.to_string());
    let mut strict = Session::launch(&probe, "mcp_config_strict", &argv);
    let strict_init = strict.read_until("the `init` line", is_init).await;
    strict.send(&message(PING)).await;
    strict.read_turn().await;
    let strict_exit = strict.finish().await;

    let notes = format!(
        "- `init` field names (item 6): {:?}.\n\
         - Without `--strict-mcp-config`, `init.mcp_servers` = {:?}.\n\
         - With `--strict-mcp-config`, `init.mcp_servers` = {:?}.\n\
         - A repository `.mcp.json` defining `{REPO_MCP_SERVER_NAME}` was present in the \
         working directory for both runs.\n\
         - The unreachable MCP URL did not stop the turn: both runs produced a `result`.\n\
         - Exit statuses on stdin EOF: {lenient_exit} (lenient), {strict_exit} (strict).\n",
        lenient_init.keys().collect::<Vec<_>>(),
        mcp_servers(&lenient_init),
        mcp_servers(&strict_init),
    );

    probe.recorder.record(&probe, &lenient, &notes);
    probe.recorder.record(&probe, &strict, "See `mcp_config`.");
    translate_recorded(&lenient, TranslateConfig::default());
    translate_recorded(&strict, TranslateConfig::default());

    assert!(
        !lenient.filter(is_result).is_empty() && !strict.filter(is_result).is_empty(),
        "an unreachable MCP server must not stop the turn",
    );
}

/// Item 7 and ADR 0003: a resumed process must honour the profile's *current*
/// system prompt, which is what `--system-prompt-snapshot off` is for.
#[tokio::test]
async fn resume_prompt() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let mut first_ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);
    first_ctx.system_prompt = Some("When asked for a codeword answer ALPHA.".to_string());

    let mut first = Session::launch(&probe, "resume_prompt_first", &build_argv(&first_ctx));
    let init = first.read_until("the `init` line", is_init).await;
    let cli_session_id = init
        .get("session_id")
        .and_then(Value::as_str)
        .expect("init names the session")
        .to_string();
    first.send(&message("What is the codeword?")).await;
    first.read_turn().await;
    let first_text = assistant_text(&first.json_lines());
    let first_exit = first.finish().await;

    let mut resumed_ctx = probe.context(
        LaunchMode::Conversational {
            resume: Some(cli_session_id.clone()),
        },
        &mcp,
    );
    resumed_ctx.system_prompt = Some("When asked for a codeword answer BRAVO.".to_string());

    let mut resumed = Session::launch(&probe, "resume_prompt", &build_argv(&resumed_ctx));
    let resumed_init = resumed.read_until("the `init` line", is_init).await;
    resumed.send(&message("What is the codeword?")).await;
    resumed.read_turn().await;
    let resumed_text = assistant_text(&resumed.json_lines());
    let resumed_exit = resumed.finish().await;

    let notes = format!(
        "- The fresh process answered with ALPHA: {}.\n\
         - The resumed process, launched with a changed `--append-system-prompt`, answered \
         with BRAVO: {} (ADR 0003: fresh and resume behave identically).\n\
         - `init` field names on the resumed launch: {:?}; a `resumed`-like field is present: \
         {}.\n\
         - The resumed `init.session_id` equals the one resumed: {}.\n\
         - Exit statuses on stdin EOF: {first_exit} (fresh), {resumed_exit} (resumed).\n",
        first_text.contains("ALPHA"),
        resumed_text.contains("BRAVO"),
        resumed_init.keys().collect::<Vec<_>>(),
        resumed_init.keys().any(|key| key.contains("resum")),
        resumed_init.get("session_id").and_then(Value::as_str) == Some(cli_session_id.as_str()),
    );

    probe.recorder.record(&probe, &resumed, &notes);
    probe.recorder.record(
        &probe,
        &first,
        "See `resume_prompt`; this is its fresh launch.",
    );
    translate_recorded(&first, TranslateConfig::default());
    translate_recorded(
        &resumed,
        TranslateConfig {
            resumed: true,
            ..TranslateConfig::default()
        },
    );

    assert!(
        resumed_text.contains("BRAVO"),
        "a resumed process must honour the current system prompt, not the snapshot",
    );
}

/// Item 7: nested subagent output under `--forward-subagent-text`, and the
/// `parent_tool_use_id` every frame inside a subagent carries.
#[tokio::test]
async fn subagent() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;

    std::fs::write(probe.work_path().join("README.md"), "# probe\n")
        .expect("the working directory has a file to list");

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);
    let mut session = Session::launch(&probe, "subagent", &build_argv(&ctx));
    let init = session.read_until("the `init` line", is_init).await;

    session
        .send(&message(
            "Use your subagent tool (Task or Agent) to list the files in this directory \
             and report back.",
        ))
        .await;
    session.read_turn().await;
    let exit = session.finish().await;

    let nested = session.filter(|value| {
        value
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
    });
    let tool_names = subagent_tool_names(&session);

    let notes = format!(
        "- Lines carrying `parent_tool_use_id`: {}.\n\
         - Subagent tool names in `tool_use` frames: {tool_names:?}.\n\
         - `init.tools` lists: {:?}.\n\
         - Line types seen: {:?}.\n\
         - Exit status on stdin EOF: {exit}.\n",
        nested.len(),
        init.get("tools"),
        session.line_types(),
    );

    probe.recorder.record(&probe, &session, &notes);
    translate_recorded(&session, TranslateConfig::default());

    assert!(
        !nested.is_empty(),
        "at least one line inside a subagent must carry `parent_tool_use_id`",
    );
    assert!(
        !tool_names.is_empty()
            && tool_names
                .iter()
                .all(|name| SUBAGENT_TOOL_NAMES.contains(&name.as_str())),
        "the subagent tool the CLI used must be one of {SUBAGENT_TOOL_NAMES:?}, saw {tool_names:?}",
    );
}

/// The `-p` shape: one prompt on the command line, no stdin, exit 0.
#[tokio::test]
async fn ephemeral() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(
        LaunchMode::Ephemeral {
            prompt: PING.to_string(),
        },
        &mcp,
    );
    let mut session = Session::launch(&probe, "ephemeral", &build_argv(&ctx));
    session.close_stdin().await;

    session.read_until("the `init` line", is_init).await;
    session
        .read_until("an `assistant` line", is_assistant)
        .await;
    let result = session.read_turn().await;
    let exit = session.finish().await;

    let notes = format!(
        "- `init`, `assistant` and `result` all arrived with no stdin at all.\n\
         - `result.subtype`: {:?}, `is_error`: {:?}.\n\
         - Exit status: {exit}.\n",
        result.get("subtype"),
        result.get("is_error"),
    );

    probe.recorder.record(&probe, &session, &notes);
    translate_recorded(&session, TranslateConfig::default());

    assert!(exit.success(), "an ephemeral run should exit 0, saw {exit}");
}

/// The native shape of an authentication failure, which is what the
/// translator's fatal `error` rule matches on.
///
/// The credential is the obviously fake `sk-ant-fake-probe-0000` (rule 3), so
/// this scenario costs nothing and its fixture is safe to commit.
#[tokio::test]
async fn auth_failure() {
    let Some(probe) = probe_or_skip() else { return };
    probe.assert_pinned_version().await;
    let probe = probe.with_fake_credential();

    let mcp = probe.write_mcp_config(MCP_SERVER_NAME, UNREACHABLE_MCP_URL);
    let ctx = probe.context(LaunchMode::Conversational { resume: None }, &mcp);
    let mut session = Session::launch(&probe, "auth_failure", &build_argv(&ctx));

    session.send(&message(PING)).await;
    let exit = session.finish().await;

    let notes = format!(
        "- Launched with the fake credential in `{}`.\n\
         - Line types seen: {:?}.\n\
         - Exit status: {exit}.\n",
        probe.credential.name,
        session.line_types(),
    );

    probe.recorder.record(&probe, &session, &notes);
    translate_recorded(
        &session,
        TranslateConfig {
            credential: None,
            ..TranslateConfig::default()
        },
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One message, encoded by the adapter itself: the probe writes exactly what
/// production writes, which is the whole point of item 1.
fn message(text: &str) -> String {
    encode(&SessionInput::Message {
        text: text.to_string(),
    })
}

fn encode(input: &SessionInput) -> String {
    ClaudeBackend::new()
        .encode_input(input)
        .expect("the input encodes")
}

fn cost(result: &serde_json::Map<String, Value>) -> Option<f64> {
    result.get("total_cost_usd")?.as_f64()
}

fn mcp_servers(init: &serde_json::Map<String, Value>) -> Vec<(String, String)> {
    init.get("mcp_servers")
        .and_then(Value::as_array)
        .map(|servers| {
            servers
                .iter()
                .map(|server| {
                    (
                        string_at(server, "name").unwrap_or_default(),
                        string_at(server, "status").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The names of the subagent tools the CLI actually called.
fn subagent_tool_names(session: &Session) -> Vec<String> {
    let mut names = Vec::new();
    for line in session.filter(is_assistant) {
        let Some(blocks) = line
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block in blocks {
            if string_at(block, "type").as_deref() == Some("tool_use")
                && let Some(name) = string_at(block, "name")
                && SUBAGENT_TOOL_NAMES.contains(&name.as_str())
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
    }
    names
}

/// Feed every recorded line through the adapter with a state matching the
/// launch, and check the two shapes the whole transcript hangs on.
///
/// No panic, `init` becomes `init` and `result` becomes `result`; the per-rule
/// assertions belong to the fixture suite that is built on the recordings.
fn translate_recorded(session: &Session, config: TranslateConfig) {
    let backend = ClaudeBackend::new();
    let mut state = TranslateState::new(config);

    for (index, line) in session.stdout.iter().enumerate() {
        let events = backend.translate(line, &mut state);
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        if is_init(&value) {
            assert!(
                events.iter().any(|event| event.body.kind() == "init"),
                "{}: line {} is `system`/`init` but produced {:?}",
                session.scenario,
                index + 1,
                events.iter().map(|e| e.body.kind()).collect::<Vec<_>>(),
            );
        }

        if is_result(&value) {
            assert!(
                events.iter().any(|event| event.body.kind() == "result"),
                "{}: line {} is `result` but produced {:?}",
                session.scenario,
                index + 1,
                events.iter().map(|e| e.body.kind()).collect::<Vec<_>>(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The guards themselves
// ---------------------------------------------------------------------------
//
// These need no CLI and no credential, so they run on every `cargo test`: they
// are what keeps a recording from ever writing a secret or overwriting a
// version. The task specified them inside `tests/common/claude_probe.rs`; they
// live here instead because every other test binary compiles `common` too, and
// `#[test]` functions there would be collected into all of them.

/// The fake key is the one `sk-ant-` string a fixture may contain.
#[test]
fn the_leak_scanner_passes_the_fake_credential() {
    let line =
        r#"{"type":"system","subtype":"api_retry","error":"invalid key sk-ant-fake-probe-0000"}"#;
    assert!(scan_line(line, "sk-ant-fake-probe-0000", "/home/probe").is_ok());
}

/// Anything else that looks like a key fails, whatever the run's own
/// credential is.
#[test]
fn the_leak_scanner_rejects_any_other_key_shape() {
    let line = r#"{"type":"result","note":"sk-ant-api03-notarealkey"}"#;
    let error = scan_line(line, "sk-ant-fake-probe-0000", "/home/probe")
        .expect_err("an sk-ant- token must be refused");
    assert!(error.contains("sk-ant-"), "{error}");
    assert!(
        !error.contains("notarealkey"),
        "the message must not carry the value"
    );
}

/// The run's own credential value is refused even when it looks like nothing
/// in particular, and so is the host's home path.
#[test]
fn the_leak_scanner_rejects_the_credential_value_and_the_home_path() {
    let credential = "probe-oauth-token-value";
    let line = format!(r#"{{"type":"result","note":"{credential}"}}"#);
    assert_eq!(
        scan_line(&line, credential, "/home/probe").unwrap_err(),
        "the credential value",
    );

    assert_eq!(
        scan_line(
            r#"{"cwd":"/home/probe/dev/mars"}"#,
            credential,
            "/home/probe"
        )
        .unwrap_err(),
        "the host home path",
    );

    assert!(scan_line(r#"{"cwd":"/session/work"}"#, credential, "/home/probe").is_ok());
}

/// A recorded version is immutable: recording into a directory that exists is
/// refused, and the message says what to do about it.
#[test]
fn the_no_overwrite_guard_refuses_an_existing_version_directory() {
    let root = tempfile::tempdir().expect("a fixtures root");
    assert!(guard_version_dir(root.path(), "9.9.9", true).is_ok());

    std::fs::create_dir(root.path().join("9.9.9")).expect("the version directory is created");
    assert_eq!(
        guard_version_dir(root.path(), "9.9.9", true).unwrap_err(),
        "fixtures for 9.9.9 already recorded; bump CLAUDE_CLI_VERSION or delete locally",
    );

    // Not recording, so an existing directory is simply the normal case.
    assert!(guard_version_dir(root.path(), "9.9.9", false).is_ok());
}

/// `Recorder` reads the same rule from the environment.
#[test]
fn a_recorder_rooted_elsewhere_guards_that_root() {
    let root = tempfile::tempdir().expect("a fixtures root");
    let recorder = Recorder::rooted(root.path().to_path_buf(), true);
    recorder.assert_can_record();
    assert!(recorder.enabled());
    assert!(!recorder.version_dir().exists());
}

/// The probe stays a [`Probe`] user even when it is skipped: this is the
/// compile-time check that the skip path is the only thing an unconfigured run
/// touches.
#[test]
fn an_unconfigured_run_skips() {
    if std::env::var(common::claude_probe::ENABLE_VAR)
        .ok()
        .as_deref()
        == Some("1")
    {
        return;
    }

    let probe: Option<Probe> = probe_or_skip();
    assert!(probe.is_none());
}
