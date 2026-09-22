//! What the pinned Claude Code CLI accepts from an MCP server on protocol
//! revision 2026-07-28, as checks over the JSON it would parse
//! (`tests/mcp_conformance.rs`).
//!
//! A hand-maintained mirror, kept to what is known to matter: the result
//! schemas bundled in Claude Code 2.1.274 (read out of the binary; its zod
//! definitions are quoted beside each check) and the two rejections that
//! reached the live stack, `pacy2` (a 403 on the `Host` check) and `4gd2v` (a
//! listing without `ttlMs` and `cacheScope`, dropped whole). Extend it only
//! from an observed rejection or from the published schema of this revision;
//! a CLI that negotiates another revision gets a module of its own beside
//! this one.

use serde_json::Value;

/// The revision this module describes, as the CLI sends it in
/// `MCP-Protocol-Version`.
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// A tool name the CLI and the Messages API accept.
pub fn is_valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Every way `answer` fails to be what the CLI accepts in reply to
/// `request`, empty when it is acceptable.
pub fn check_answer(request: &Value, answer: &Value) -> Vec<String> {
    let mut problems = Vec::new();

    if answer["jsonrpc"] != "2.0" {
        problems.push(format!("jsonrpc is {}, not \"2.0\"", answer["jsonrpc"]));
    }
    if answer.get("id") != request.get("id") {
        problems.push(format!(
            "id is {}, the request's is {}",
            answer["id"], request["id"]
        ));
    }

    match (answer.get("result"), answer.get("error")) {
        (Some(result), None) => {
            let method = request["method"].as_str().unwrap_or_default();
            check_result(method, result, &mut problems);
        }
        (None, Some(error)) => check_error(error, &mut problems),
        _ => problems.push("exactly one of result and error".to_string()),
    }

    problems
}

/// A JSON-RPC error object: `{ code: integer, message: string, data? }`.
fn check_error(error: &Value, problems: &mut Vec<String>) {
    if !error["code"].is_i64() {
        problems.push(format!("error.code is {}, not an integer", error["code"]));
    }
    if !error["message"].is_string() {
        problems.push(format!(
            "error.message is {}, not a string",
            error["message"]
        ));
    }
}

/// `ve(he) = it({ _meta, resultType: o().default("complete"), ...he })`: the
/// CLI fills in a missing `resultType`, but the revision makes the server
/// send it, and anything but `complete` is refused for every method Mars
/// serves (`Handler for <method> returned resultType ..., but results of
/// <method> only support 'complete' on protocol revision 2026-07-28`).
fn check_result(method: &str, result: &Value, problems: &mut Vec<String>) {
    if !result.is_object() {
        problems.push(format!("result is {result}, not an object"));
        return;
    }
    if result["resultType"] != "complete" {
        problems.push(format!(
            "resultType is {}, not \"complete\"",
            result["resultType"]
        ));
    }

    match method {
        "server/discover" => check_discover(result, problems),
        "tools/list" => check_tool_list(result, problems),
        "tools/call" => check_call_result(result, problems),
        _ => {}
    }
}

/// `supportedVersions: C(o()), capabilities: _He, instructions:
/// o().optional()`, and this revision among the versions: the CLI pins it and
/// has no fallback.
fn check_discover(result: &Value, problems: &mut Vec<String>) {
    match result["supportedVersions"].as_array() {
        Some(versions) if versions.iter().all(Value::is_string) => {
            if !versions.iter().any(|version| version == PROTOCOL_VERSION) {
                problems.push(format!(
                    "supportedVersions {versions:?} lacks {PROTOCOL_VERSION}"
                ));
            }
        }
        _ => problems.push("supportedVersions is not an array of strings".to_string()),
    }
    if !result["capabilities"].is_object() {
        problems.push("capabilities is not an object".to_string());
    }
    if !result["capabilities"]["tools"].is_object() {
        problems.push("capabilities advertise no tools".to_string());
    }
    optional_string(result, "instructions", problems);
}

/// `ttlMs: E().int().min(0), cacheScope: q(["public","private"]), tools:
/// C(Ge), nextCursor: s.optional()`. One tool that fails `Ge` fails the whole
/// listing, which is how `4gd2v` looked from the session: no tools at all.
fn check_tool_list(result: &Value, problems: &mut Vec<String>) {
    if !result["ttlMs"].is_u64() {
        problems.push(format!(
            "ttlMs is {}, not a non-negative integer",
            result["ttlMs"]
        ));
    }
    if !matches!(result["cacheScope"].as_str(), Some("public" | "private")) {
        problems.push(format!(
            "cacheScope is {}, not \"public\" or \"private\"",
            result["cacheScope"]
        ));
    }
    optional_string(result, "nextCursor", problems);

    let Some(tools) = result["tools"].as_array() else {
        problems.push("tools is not an array".to_string());
        return;
    };
    for tool in tools {
        check_tool(tool, problems);
    }
}

/// `Ge = u({ name, title?, description: o().optional(), inputSchema: it({
/// $schema: o().optional(), type: R("object") }), outputSchema: it({ $schema:
/// o().optional() }).optional(), annotations?, _meta? })`. The output schema
/// is held to `type: "object"` as well, which the revision's own schema
/// requires even where this CLI does not check it.
pub fn check_tool(tool: &Value, problems: &mut Vec<String>) {
    let name = tool["name"].as_str().unwrap_or_default();
    if !is_valid_tool_name(name) {
        problems.push(format!("tool name {} is not a valid name", tool["name"]));
    }
    optional_string(tool, "description", problems);

    for (key, required) in [("inputSchema", true), ("outputSchema", false)] {
        match tool.get(key) {
            None if !required => {}
            Some(schema) if schema.is_object() && schema["type"] == "object" => {
                optional_string(schema, "$schema", problems);
            }
            other => problems.push(format!(
                "{name}: {key} is {other:?}, not an object schema with \"type\": \"object\""
            )),
        }
    }
}

/// `content: C(O), structuredContent: ae().optional(), isError:
/// H().optional()`.
fn check_call_result(result: &Value, problems: &mut Vec<String>) {
    match result["content"].as_array() {
        Some(content) => {
            for block in content {
                if !block["type"].is_string() {
                    problems.push(format!("content block {block} has no type"));
                }
            }
        }
        None => problems.push("content is not an array".to_string()),
    }
    if let Some(is_error) = result.get("isError")
        && !is_error.is_boolean()
    {
        problems.push(format!("isError is {is_error}, not a boolean"));
    }
}

fn optional_string(object: &Value, key: &str, problems: &mut Vec<String>) {
    if let Some(value) = object.get(key)
        && !value.is_string()
    {
        problems.push(format!("{key} is {value}, not a string"));
    }
}
