//! The tool descriptions in the code are the tool descriptions in `SPEC.md`.
//!
//! "Tool descriptions are part of the contract because they steer the agent.
//! They are reproduced verbatim" (`SPEC.md`, "MCP tool contracts"). This test
//! parses the document itself, so the constants cannot drift from it: an
//! edited sentence, a changed backtick or a stray full stop fails here, and
//! the document wins.
//!
//! It also counts the headings, so adding a thirteenth tool to the document
//! without adding it to [`ToolName`] fails rather than passing unnoticed.
//!
//! The server's `instructions` string is held to the same standard, and both
//! it and the descriptions are checked for the name of a tracker product,
//! which would make the text untrue for every repository that uses a different
//! one.
//!
//! No database, no container engine: it reads a file and compares strings.

use mars_orchestrator::mcp::{INSTRUCTIONS, ToolName};

/// Only the deny-list, not the whole of `common`: this binary needs neither a
/// database nor the `integration-tests` mocks and is not going to grow them.
#[path = "common/tracker_products.rs"]
mod tracker_products;

use tracker_products::names_a_tracker_product;

/// The document, read at compile time from beside the crate.
const SPEC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../SPEC.md"));

/// The heading the tool subsections live under.
const SECTION: &str = "## MCP tool contracts";

/// The lines of "MCP tool contracts", from its heading to the next `##`.
fn section() -> Vec<&'static str> {
    let body = SPEC
        .split_once(SECTION)
        .expect("SPEC.md has an \"MCP tool contracts\" section")
        .1;
    let body = body.split_once("\n## ").map_or(body, |(before, _)| before);

    body.lines().collect()
}

/// The `Description: "..."` line that follows a tool's heading.
///
/// The text is what lies between the first and the last double quote of that
/// line, so a description containing a quoted phrase would survive; nothing is
/// trimmed, unescaped or reflowed.
fn documented_description(tool: ToolName) -> String {
    let lines = section();
    let heading = format!("### `{}`", tool.as_str());

    let start = lines
        .iter()
        .position(|line| line.starts_with(&heading))
        .unwrap_or_else(|| panic!("SPEC.md has a `{}` subsection", tool.as_str()));

    let line = lines[start..]
        .iter()
        .find(|line| line.starts_with("Description: \""))
        .unwrap_or_else(|| panic!("`{}` has a description line", tool.as_str()));

    let first = line.find('"').expect("an opening quote");
    let last = line.rfind('"').expect("a closing quote");
    assert!(last > first, "`{}`: an empty description", tool.as_str());

    line[first + 1..last].to_string()
}

/// The one blockquote of "MCP tool contracts": the server's `instructions`.
fn documented_instructions() -> String {
    let quotes: Vec<&str> = section()
        .into_iter()
        .filter(|line| line.starts_with("> "))
        .collect();

    assert_eq!(
        quotes.len(),
        1,
        "\"MCP tool contracts\" quotes the instructions in exactly one blockquote line: {quotes:?}",
    );

    quotes[0][2..].to_string()
}

#[test]
fn the_instructions_match_the_document_byte_for_byte() {
    assert_eq!(
        INSTRUCTIONS,
        documented_instructions(),
        "the constant and SPEC.md disagree; the document wins",
    );
}

#[test]
fn no_agent_facing_text_names_a_tracker_product() {
    // The instructions claim to replace "any task tracker ... the repository's
    // own instructions name". Naming one would send the agent looking for it.
    assert_eq!(
        names_a_tracker_product(INSTRUCTIONS),
        None,
        "the MCP instructions name a tracker product",
    );

    for tool in ToolName::ALL {
        assert_eq!(
            names_a_tracker_product(tool.description()),
            None,
            "`{}`'s description names a tracker product",
            tool.as_str(),
        );
    }
}

#[test]
fn every_description_matches_the_document_byte_for_byte() {
    for tool in ToolName::ALL {
        let documented = documented_description(tool);

        assert_eq!(
            tool.description(),
            documented,
            "`{}`: the constant and SPEC.md disagree; the document wins",
            tool.as_str(),
        );
    }
}

#[test]
fn the_document_defines_exactly_the_tools_the_code_knows() {
    let headings: Vec<&str> = section()
        .into_iter()
        .filter(|line| line.starts_with("### `"))
        .collect();

    assert_eq!(
        headings.len(),
        ToolName::ALL.len(),
        "SPEC.md documents {} tools, the code knows {}: {headings:?}",
        headings.len(),
        ToolName::ALL.len(),
    );

    // The same tools, in the same order: `tools/list` answers in the
    // document's order.
    let documented: Vec<&str> = headings
        .iter()
        .map(|line| line.split('`').nth(1).expect("a backticked tool name"))
        .collect();

    assert_eq!(documented, ToolName::ALL.map(ToolName::as_str).to_vec());
}
