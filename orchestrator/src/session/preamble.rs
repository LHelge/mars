//! The Mars-owned paragraph every session's system prompt begins with.
//!
//! A profile's `system_prompt` is the project's own text: copied from a
//! template at creation or written by a user, and never read from a template
//! again (`SPEC.md`, "Role profile templates"). What a session may and may not
//! do with git is not the project's to word, though, and a sentence in the
//! templates would reach neither an existing project nor a profile a user
//! wrote. So the launcher composes one fixed paragraph in front of the
//! profile's prompt, on every launch, whatever the profile, project or backend
//! (`SPEC.md`, "Session preamble"; ADR 0055).
//!
//! The text lives beside the role templates as `projects/templates/preamble.md`,
//! embedded with [`include_str!`] like them, and `SPEC.md` reproduces it
//! verbatim; `tests/profile_templates.rs` compares the two. Its one placeholder,
//! [`SESSION_ID_PLACEHOLDER`], becomes the session's id, so the branch it names
//! is the `session/<sid>` the work clone is checked out on
//! (`ARCHITECTURE.md`, "Git model" → "Session clone").

use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// The preamble as written, with its placeholder in place.
pub const SESSION_PREAMBLE: &str = include_str!("../projects/templates/preamble.md");

/// What [`SESSION_PREAMBLE`] says where the session's id goes.
pub const SESSION_ID_PLACEHOLDER: &str = "{session_id}";

/// The preamble for `session_id`, without the file's trailing newline.
pub fn session_preamble(session_id: Uuid) -> String {
    SESSION_PREAMBLE
        .trim_end()
        .replace(SESSION_ID_PLACEHOLDER, &session_id.to_string())
}

/// The system prompt a launch of `session_id` passes to its backend: the
/// preamble, then a blank line, then the profile's own prompt.
///
/// A profile with no prompt, or a blank one, gets the preamble alone, so a
/// launch always has a system prompt. The profile's text is passed as it is
/// stored: it is prose a person wrote, whitespace included.
pub fn session_system_prompt(session_id: Uuid, profile_prompt: Option<&str>) -> String {
    let preamble = session_preamble(session_id);

    match profile_prompt {
        Some(prompt) if !prompt.trim().is_empty() => format!("{preamble}\n\n{prompt}"),
        _ => preamble,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid() -> Uuid {
        Uuid::parse_str("0b6f7c1e-2d3a-4e5f-8a9b-0c1d2e3f4a5b").expect("a valid uuid")
    }

    #[test]
    fn the_preamble_comes_first_and_a_blank_line_separates_it_from_the_profile() {
        let prompt = session_system_prompt(sid(), Some("You are the planner.\n"));

        assert_eq!(
            prompt,
            format!("{}\n\nYou are the planner.\n", session_preamble(sid())),
        );
        assert!(prompt.starts_with("You are running inside Mars."));
    }

    #[test]
    fn a_profile_without_a_prompt_gets_the_preamble_alone() {
        for profile_prompt in [None, Some(""), Some("  \n\t\n")] {
            assert_eq!(
                session_system_prompt(sid(), profile_prompt),
                session_preamble(sid()),
                "{profile_prompt:?}",
            );
        }
    }

    #[test]
    fn the_branch_named_is_the_sessions_own() {
        let preamble = session_preamble(sid());

        assert!(
            preamble.contains(&format!("`{}`", crate::git::session_branch(sid()))),
            "{preamble}",
        );
        assert!(!preamble.contains(SESSION_ID_PLACEHOLDER));
    }

    #[test]
    fn the_file_carries_exactly_one_placeholder() {
        assert_eq!(SESSION_PREAMBLE.matches(SESSION_ID_PLACEHOLDER).count(), 1);
        assert!(!SESSION_PREAMBLE.trim_end().ends_with('\n'));
    }

    #[test]
    fn the_preamble_and_the_longest_profile_prompt_fit_an_argument() {
        // Linux caps one argv string at 128 KiB (MAX_ARG_STRLEN); the profile
        // prompt is capped at 64 KiB, so the preamble has room to spare.
        assert!(SESSION_PREAMBLE.len() < 4 * 1024);
    }
}
