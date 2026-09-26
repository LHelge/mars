//! The role prompts in the code are the role prompts in `SPEC.md`.
//!
//! "The texts below are the templates verbatim" (`SPEC.md`, "Role profile
//! templates"). This test parses the document itself, so the embedded files
//! cannot drift from it: a reworded sentence, a changed backtick or a lost
//! paragraph fails here and the document wins. It is the same arrangement
//! `tests/mcp_descriptions.rs` has for the tool descriptions. A scheduled
//! template has two texts — the system prompt and the prompt each run is
//! given — and the subsection's fenced blocks are read in that order.
//!
//! It also checks the table beside those texts — kind, served states, tool
//! lists, the schedule expression, which ones auto-launch, which one is the
//! default and which ones creation seeds — and runs the deny-list of
//! `tests/common/tracker_products.rs` over each prompt: a prompt tells the
//! agent to use the task tools of its own session, and a product name in it
//! would send the agent looking for something else.
//!
//! The session preamble every launch puts in front of a profile's prompt
//! (`SPEC.md`, "Session preamble") lives beside the templates and is held to
//! the same two rules: its file is the document's block byte for byte, and it
//! names no tracker product.
//!
//! No database, no container engine: it reads a file and compares strings.

use mars_orchestrator::models::ProfileKind;
use mars_orchestrator::projects::profile_templates;
use mars_orchestrator::session::SESSION_PREAMBLE;

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

/// The fenced blocks of a template's subsection, in the order they appear,
/// each joined back into the text its file is supposed to hold.
///
/// Nothing is trimmed, unescaped or reflowed: the lines between an opening and
/// its closing fence are the prompt, and the trailing newline every text file
/// ends with is added back. A template a person launches has one block, a
/// scheduled one has two — the system prompt and then the run prompt.
fn documented_blocks(name: &str) -> Vec<String> {
    let lines = section();
    let heading = format!("### `{name}`");

    let start = lines
        .iter()
        .position(|line| line.starts_with(&heading))
        .unwrap_or_else(|| panic!("SPEC.md has a `{name}` subsection"));
    let end = start
        + 1
        + lines[start + 1..]
            .iter()
            .position(|line| line.starts_with("### "))
            .unwrap_or(lines.len() - start - 1);

    let mut blocks = Vec::new();
    let mut rest = &lines[start..end];

    while let Some(open) = rest.iter().position(|line| *line == "```text") {
        let body = &rest[open + 1..];
        let close = body
            .iter()
            .position(|line| *line == "```")
            .unwrap_or_else(|| panic!("`{name}`'s block is closed"));

        let mut prompt = body[..close].join("\n");
        prompt.push('\n');
        blocks.push(prompt);

        rest = &body[close + 1..];
    }

    assert!(!blocks.is_empty(), "`{name}` has a ```text block");

    blocks
}

/// The system prompt of `name`, which is its subsection's first block.
fn documented_prompt(name: &str) -> String {
    documented_blocks(name)
        .into_iter()
        .next()
        .expect("a subsection has at least one block")
}

/// The `| name | kind | serves | tools | cron | auto | default | seeded |` row
/// of the table, as its eight cells.
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

/// A kind as the document spells it, which is also how the API serialises it.
///
/// A `match` and not a cast: a kind added to the enum fails to compile here
/// until the table says what it is called.
fn kind_name(kind: ProfileKind) -> &'static str {
    match kind {
        ProfileKind::Conversational => "conversational",
        ProfileKind::Ephemeral => "ephemeral",
    }
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
fn every_schedule_prompt_matches_the_document_byte_for_byte() {
    for template in profile_templates() {
        let blocks = documented_blocks(template.name);

        match template.schedule_prompt {
            Some(prompt) => {
                assert_eq!(
                    blocks.len(),
                    2,
                    "`{}` is scheduled, so its subsection holds its system \
                     prompt and its run prompt",
                    template.name,
                );
                assert_eq!(
                    Some(prompt),
                    blocks.get(1).map(String::as_str),
                    "`{}`: the schedule prompt and SPEC.md disagree; the \
                     document wins",
                    template.name,
                );
            }
            None => assert_eq!(
                blocks.len(),
                1,
                "`{}` has no schedule, so its subsection holds one text",
                template.name,
            ),
        }
    }
}

#[test]
fn the_document_defines_exactly_the_templates_the_code_knows() {
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
        assert_eq!(row.len(), 8, "`{}`: eight cells", template.name);

        assert_eq!(
            cell_names(&row[1]),
            owned(&[kind_name(template.kind)]),
            "`{}`: kind",
            template.name,
        );
        assert_eq!(
            cell_names(&row[2]),
            owned(template.serves_states),
            "`{}`: served states",
            template.name,
        );
        assert_eq!(
            cell_names(&row[3]),
            owned(template.mcp_tools),
            "`{}`: tools",
            template.name,
        );
        assert_eq!(
            cell_names(&row[4]),
            owned(&template.schedule_cron.into_iter().collect::<Vec<_>>()),
            "`{}`: the cron expression",
            template.name,
        );
        assert_eq!(
            row[5],
            if template.auto_launch { "yes" } else { "no" },
            "`{}`: the auto-launch flag",
            template.name,
        );
        assert_eq!(
            row[6],
            if template.is_default { "yes" } else { "no" },
            "`{}`: the default flag",
            template.name,
        );
        assert_eq!(
            row[7],
            if template.seeded { "yes" } else { "no" },
            "`{}`: whether project creation seeds it",
            template.name,
        );
    }
}

/// The heading the session preamble lives under.
const PREAMBLE_SECTION: &str = "## Session preamble";

/// The one fenced block of `SPEC.md`, "Session preamble", as the text its file
/// is supposed to hold — read the way [`documented_blocks`] reads a template's.
fn documented_preamble() -> String {
    let body = SPEC
        .split_once(PREAMBLE_SECTION)
        .expect("SPEC.md has a \"Session preamble\" section")
        .1;
    let body = body.split_once("\n## ").map_or(body, |(before, _)| before);
    let lines: Vec<&str> = body.lines().collect();

    let open = lines
        .iter()
        .position(|line| *line == "```text")
        .expect("the preamble has a ```text block");
    let close = open
        + 1
        + lines[open + 1..]
            .iter()
            .position(|line| *line == "```")
            .expect("the preamble's block is closed");

    let mut text = lines[open + 1..close].join("\n");
    text.push('\n');

    text
}

#[test]
fn the_session_preamble_matches_the_document_byte_for_byte() {
    assert_eq!(
        SESSION_PREAMBLE,
        documented_preamble(),
        "the preamble file and SPEC.md disagree; the document wins",
    );
}

#[test]
fn the_session_preamble_names_no_task_tracker_product() {
    if let Some(product) = names_a_tracker_product(SESSION_PREAMBLE) {
        panic!(
            "the session preamble names {product}; it is about git and is sent \
             to every session of every project",
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
