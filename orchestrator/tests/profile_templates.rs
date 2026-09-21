//! The role prompts in the code are the role prompts in `SPEC.md`.
//!
//! "The four texts below are the templates verbatim" (`SPEC.md`, "Role profile
//! templates"). This test parses the document itself, so the embedded files
//! cannot drift from it: a reworded sentence, a changed backtick or a lost
//! paragraph fails here and the document wins. It is the same arrangement
//! `tests/mcp_descriptions.rs` has for the tool descriptions.
//!
//! It also checks the table beside those texts — served states, tool lists and
//! which one is the default — and runs the deny-list of
//! `tests/common/tracker_products.rs` over each prompt: a prompt tells the
//! agent to use the task tools of its own session, and a product name in it
//! would send the agent looking for something else.
//!
//! No database, no container engine: it reads a file and compares strings.

use mars_orchestrator::projects::profile_templates;

// The one deny-list, shared with `tests/mcp_descriptions.rs`: the MCP
// instructions, the tool descriptions and these prompts are all agent-facing
// text under the same rule.
#[path = "common/tracker_products.rs"]
mod tracker_products;

use tracker_products::names_a_tracker_product;

/// The document, read at compile time from beside the crate.
const SPEC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../SPEC.md"));

/// The heading the prompts and the table live under.
const SECTION: &str = "## Role profile templates";

/// The lines of "Role profile templates", from its heading to the next `##`.
fn section() -> Vec<&'static str> {
    let body = SPEC
        .split_once(SECTION)
        .expect("SPEC.md has a \"Role profile templates\" section")
        .1;
    let body = body.split_once("\n## ").map_or(body, |(before, _)| before);

    body.lines().collect()
}

/// The fenced block that follows a template's heading, joined back into the
/// text the file is supposed to hold.
///
/// Nothing is trimmed, unescaped or reflowed: the lines between the opening
/// and closing fences are the prompt, and the trailing newline every text file
/// ends with is added back.
fn documented_prompt(name: &str) -> String {
    let lines = section();
    let heading = format!("### `{name}`");

    let start = lines
        .iter()
        .position(|line| line.starts_with(&heading))
        .unwrap_or_else(|| panic!("SPEC.md has a `{name}` subsection"));

    let open = start
        + lines[start..]
            .iter()
            .position(|line| *line == "```text")
            .unwrap_or_else(|| panic!("`{name}` has a ```text block"))
        + 1;
    let close = open
        + lines[open..]
            .iter()
            .position(|line| *line == "```")
            .unwrap_or_else(|| panic!("`{name}`'s block is closed"));

    let mut prompt = lines[open..close].join("\n");
    prompt.push('\n');

    prompt
}

/// The `| name | serves | tools | default |` row of the table, as its four
/// cells.
fn documented_row(name: &str) -> Vec<String> {
    let cell = format!("| `{name}` |");
    let line = section()
        .into_iter()
        .find(|line| line.starts_with(&cell))
        .unwrap_or_else(|| panic!("SPEC.md's table has a `{name}` row"));

    line.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// A table cell of backticked names as the list it stands for; `—` is empty.
fn cell_names(cell: &str) -> Vec<String> {
    if cell == "—" {
        return Vec::new();
    }

    cell.split(',')
        .map(|entry| entry.trim().trim_matches('`').to_string())
        .collect()
}

/// A list of static names as owned strings, to compare with parsed cells.
fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn every_prompt_matches_the_document_byte_for_byte() {
    for template in profile_templates() {
        assert_eq!(
            template.system_prompt,
            documented_prompt(template.name),
            "`{}`: the template file and SPEC.md disagree; the document wins",
            template.name,
        );
    }
}

#[test]
fn the_document_defines_exactly_the_four_templates_the_code_knows() {
    let headings: Vec<String> = section()
        .into_iter()
        .filter(|line| line.starts_with("### `"))
        .map(str::to_string)
        .collect();

    let expected: Vec<String> = profile_templates()
        .iter()
        .map(|template| format!("### `{}`", template.name))
        .collect();

    assert_eq!(headings, expected);
}

#[test]
fn the_table_says_what_the_templates_say() {
    for template in profile_templates() {
        let row = documented_row(template.name);
        assert_eq!(row.len(), 4, "`{}`: four cells", template.name);

        assert_eq!(
            cell_names(&row[1]),
            owned(template.serves_states),
            "`{}`: served states",
            template.name,
        );
        assert_eq!(
            cell_names(&row[2]),
            owned(template.mcp_tools),
            "`{}`: tools",
            template.name,
        );
        assert_eq!(
            row[3],
            if template.is_default { "yes" } else { "no" },
            "`{}`: the default flag",
            template.name,
        );
    }
}

#[test]
fn no_prompt_names_a_task_tracker_product() {
    for template in profile_templates() {
        if let Some(product) = names_a_tracker_product(template.system_prompt) {
            panic!(
                "`{}` names {product}; a prompt says \"the task tools of this \
                 session\" and never a product, because it has to hold for \
                 whatever repository the session was launched against",
                template.name,
            );
        }
    }
}
