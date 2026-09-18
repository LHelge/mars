//! The `claude` argument vector for the three launch shapes
//! (`ARCHITECTURE.md`, "Claude Code invocation" and "Launch sequence").
//!
//! One function builds the whole command line so the container `Cmd` is
//! deterministic: every documented flag is emitted in a fixed order, the
//! optional groups appear only when the [`LaunchContext`] sets them, and the
//! fixture and probe tests can compare the result byte for byte.
//!
//! The entrypoint receives argv, not a shell string
//! (`ARCHITECTURE.md`, "Session image"): the prompt, the model and the system
//! prompt are each one element of the vector and are never quoted, escaped or
//! joined with anything. A value containing spaces, quotes or newlines is
//! passed through exactly as the profile stored it.
//!
//! [`LaunchMode`] makes an ephemeral launch with a resume id unrepresentable,
//! so nothing here validates that combination at runtime.
//!
//! Prompt length is the launcher's concern. The CLI receives the ephemeral
//! prompt as a command-line argument and v1 sets no limit, but a very long
//! prompt (above roughly 128 KiB) approaches the kernel's per-argument limit
//! and should be trimmed before it reaches this function.

use super::super::{Command, LaunchContext, LaunchMode};

/// The CLI binary as it is found on the session image's `PATH`.
pub const BINARY: &str = "claude";

/// Streaming protocol for the conversational shape; `-p` for the ephemeral
/// one, which is the same flag's short form carrying the prompt.
pub const PRINT: &str = "--print";
/// The ephemeral form of [`PRINT`], with the prompt as its value.
pub const PRINT_SHORT: &str = "-p";

pub const OUTPUT_FORMAT_FLAG: &str = "--output-format";
pub const OUTPUT_FORMAT: &str = "stream-json";
pub const INPUT_FORMAT_FLAG: &str = "--input-format";
pub const INPUT_FORMAT: &str = "stream-json";
pub const VERBOSE: &str = "--verbose";
pub const FORWARD_SUBAGENT_TEXT: &str = "--forward-subagent-text";
pub const SYSTEM_PROMPT_SNAPSHOT_FLAG: &str = "--system-prompt-snapshot";
pub const SYSTEM_PROMPT_SNAPSHOT: &str = "off";
pub const INCLUDE_PARTIAL_MESSAGES: &str = "--include-partial-messages";
pub const PERMISSION_MODE_FLAG: &str = "--permission-mode";
pub const PERMISSION_MODE: &str = "bypassPermissions";
pub const PERMISSION_PROMPTS_FLAG: &str = "--permission-prompts";
pub const PERMISSION_PROMPTS: &str = "none";
pub const MCP_CONFIG_FLAG: &str = "--mcp-config";
pub const MODEL_FLAG: &str = "--model";
pub const APPEND_SYSTEM_PROMPT_FLAG: &str = "--append-system-prompt";
pub const RESUME_FLAG: &str = "--resume";

/// Build the launch command's argv for `ctx`.
///
/// Order is fixed and identical for both modes after the leading protocol
/// flags: output and verbosity, subagent text, prompt snapshot, the optional
/// partial-messages flag, permissions, the MCP configuration, then the
/// optional model, system prompt and resume id.
///
/// `--bare` and `--strict-mcp-config` are never emitted
/// (`ARCHITECTURE.md`, "Claude Code invocation").
pub(crate) fn build_argv(ctx: &LaunchContext) -> Vec<String> {
    let mut argv = vec![BINARY.to_string()];

    match &ctx.mode {
        LaunchMode::Conversational { .. } => {
            argv.push(PRINT.to_string());
            argv.push(OUTPUT_FORMAT_FLAG.to_string());
            argv.push(OUTPUT_FORMAT.to_string());
            argv.push(INPUT_FORMAT_FLAG.to_string());
            argv.push(INPUT_FORMAT.to_string());
        }
        LaunchMode::Ephemeral { prompt } => {
            argv.push(PRINT_SHORT.to_string());
            argv.push(prompt.clone());
            argv.push(OUTPUT_FORMAT_FLAG.to_string());
            argv.push(OUTPUT_FORMAT.to_string());
        }
    }

    argv.push(VERBOSE.to_string());
    argv.push(FORWARD_SUBAGENT_TEXT.to_string());
    argv.push(SYSTEM_PROMPT_SNAPSHOT_FLAG.to_string());
    argv.push(SYSTEM_PROMPT_SNAPSHOT.to_string());

    if ctx.partial_messages {
        argv.push(INCLUDE_PARTIAL_MESSAGES.to_string());
    }

    argv.push(PERMISSION_MODE_FLAG.to_string());
    argv.push(PERMISSION_MODE.to_string());
    argv.push(PERMISSION_PROMPTS_FLAG.to_string());
    argv.push(PERMISSION_PROMPTS.to_string());
    argv.push(MCP_CONFIG_FLAG.to_string());
    argv.push(ctx.mcp_config_path.clone());

    if let Some(model) = set(ctx.model.as_deref()) {
        argv.push(MODEL_FLAG.to_string());
        argv.push(model.to_string());
    }

    if let Some(system_prompt) = set(ctx.system_prompt.as_deref()) {
        argv.push(APPEND_SYSTEM_PROMPT_FLAG.to_string());
        argv.push(system_prompt.to_string());
    }

    if let Some(resume) = set(ctx.resume()) {
        argv.push(RESUME_FLAG.to_string());
        argv.push(resume.to_string());
    }

    argv
}

/// The launch command for `ctx`.
pub(crate) fn launch_command(ctx: &LaunchContext) -> Command {
    Command {
        argv: build_argv(ctx),
    }
}

/// An optional value the profile actually set: an empty string is unset, so a
/// blank `model` or `system_prompt` column never produces a flag with an empty
/// argument.
fn set(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(ctx: &LaunchContext) -> Vec<String> {
        build_argv(ctx)
    }

    fn conversational() -> LaunchContext {
        LaunchContext::new(LaunchMode::Conversational { resume: None })
    }

    #[test]
    fn a_fresh_conversational_launch_has_the_documented_flags() {
        let mut ctx = conversational();
        ctx.partial_messages = true;

        assert_eq!(
            argv(&ctx),
            vec![
                "claude",
                "--print",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--forward-subagent-text",
                "--system-prompt-snapshot",
                "off",
                "--include-partial-messages",
                "--permission-mode",
                "bypassPermissions",
                "--permission-prompts",
                "none",
                "--mcp-config",
                "/session/mcp.json",
            ],
        );
    }

    #[test]
    fn a_model_and_a_system_prompt_come_last_and_partial_messages_can_be_off() {
        let mut ctx = conversational();
        ctx.model = Some("claude-sonnet-4-5".to_string());
        ctx.system_prompt = Some("You are \"terse\".\nAlways.".to_string());

        assert_eq!(
            argv(&ctx),
            vec![
                "claude",
                "--print",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--forward-subagent-text",
                "--system-prompt-snapshot",
                "off",
                "--permission-mode",
                "bypassPermissions",
                "--permission-prompts",
                "none",
                "--mcp-config",
                "/session/mcp.json",
                "--model",
                "claude-sonnet-4-5",
                "--append-system-prompt",
                "You are \"terse\".\nAlways.",
            ],
        );
    }

    #[test]
    fn a_resume_is_the_last_two_elements() {
        let mut ctx = LaunchContext::new(LaunchMode::Conversational {
            resume: Some("cli-session-1".to_string()),
        });
        ctx.model = Some("opus".to_string());
        ctx.system_prompt = Some("do the thing".to_string());

        let argv = argv(&ctx);
        assert_eq!(
            argv[argv.len() - 2..],
            ["--resume".to_string(), "cli-session-1".to_string()],
        );
        assert_eq!(argv.iter().filter(|a| *a == "--resume").count(), 1);
    }

    #[test]
    fn an_ephemeral_launch_carries_its_prompt_as_one_element() {
        let mut ctx = LaunchContext::new(LaunchMode::Ephemeral {
            prompt: "Task: fix the flake\n\nIt fails on CI only.".to_string(),
        });
        ctx.partial_messages = true;
        ctx.model = Some("haiku".to_string());
        ctx.system_prompt = Some("be brief".to_string());

        assert_eq!(
            argv(&ctx),
            vec![
                "claude",
                "-p",
                "Task: fix the flake\n\nIt fails on CI only.",
                "--output-format",
                "stream-json",
                "--verbose",
                "--forward-subagent-text",
                "--system-prompt-snapshot",
                "off",
                "--include-partial-messages",
                "--permission-mode",
                "bypassPermissions",
                "--permission-prompts",
                "none",
                "--mcp-config",
                "/session/mcp.json",
                "--model",
                "haiku",
                "--append-system-prompt",
                "be brief",
            ],
        );
    }

    #[test]
    fn an_ephemeral_launch_never_prints_resumes_or_reads_stdin() {
        let ctx = LaunchContext::new(LaunchMode::Ephemeral {
            prompt: "one shot".to_string(),
        });
        let argv = argv(&ctx);

        assert!(!argv.contains(&"--print".to_string()));
        assert!(!argv.contains(&"--input-format".to_string()));
        assert!(!argv.contains(&"--resume".to_string()));
    }

    #[test]
    fn neither_bare_nor_strict_mcp_config_is_ever_passed() {
        let contexts = [
            conversational(),
            LaunchContext::new(LaunchMode::Conversational {
                resume: Some("cli-session-1".to_string()),
            }),
            LaunchContext::new(LaunchMode::Ephemeral {
                prompt: "one shot".to_string(),
            }),
        ];

        for ctx in contexts {
            let argv = argv(&ctx);
            assert!(!argv.contains(&"--bare".to_string()), "{argv:?}");
            assert!(
                !argv.contains(&"--strict-mcp-config".to_string()),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn empty_optional_values_are_treated_as_unset() {
        let mut ctx = LaunchContext::new(LaunchMode::Conversational {
            resume: Some(String::new()),
        });
        ctx.model = Some(String::new());
        ctx.system_prompt = Some(String::new());

        let argv = argv(&ctx);
        assert!(!argv.contains(&"--model".to_string()));
        assert!(!argv.contains(&"--append-system-prompt".to_string()));
        assert!(!argv.contains(&"--resume".to_string()));
        assert_eq!(argv.last(), Some(&"/session/mcp.json".to_string()));
    }

    #[test]
    fn the_mcp_config_path_comes_from_the_context() {
        let mut ctx = conversational();
        ctx.mcp_config_path = "/session/other.json".to_string();

        let argv = argv(&ctx);
        let flag = argv.iter().position(|a| a == "--mcp-config").unwrap();
        assert_eq!(argv[flag + 1], "/session/other.json");
    }
}
