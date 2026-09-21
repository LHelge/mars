//! The names of tracker products that Mars's own agent-facing text must not
//! contain.
//!
//! Anything an agent reads as instruction — the MCP server's `instructions`,
//! the tool descriptions, the seeded profile prompts — has to hold for
//! whatever repository the session was launched against, so it says "the task
//! tracker the repository's instructions name" and never a product. Naming one
//! is worse than saying nothing: the agent goes looking for it.
//!
//! Deliberately short. It is a tripwire for the obvious mistake, not an
//! inventory of the market, and every entry is a word that has no other
//! meaning in this text.

/// Matched case-insensitively as a substring by [`names_a_tracker_product`].
pub const TRACKER_PRODUCTS: &[&str] = &[
    "Jira",
    "Linear",
    "Asana",
    "Trello",
    "Notion",
    "GitHub Issues",
    "Bears",
];

/// The first entry of [`TRACKER_PRODUCTS`] that `text` contains, if any.
pub fn names_a_tracker_product(text: &str) -> Option<&'static str> {
    let haystack = text.to_lowercase();

    TRACKER_PRODUCTS
        .iter()
        .copied()
        .find(|product| haystack.contains(&product.to_lowercase()))
}
