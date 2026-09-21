//! The four role profiles every project is created with.
//!
//! A role in Mars is not code: it is a profile's served states plus its system
//! prompt (`ARCHITECTURE.md`, "Task tracker" → "State is a queue"). A project
//! that started with one blank agent left every user to write those four
//! prompts before anything could flow through the board, so creation seeds
//! them: a planner over `backlog`, an implementer over `ready`, a reviewer
//! over `review` and a merger over `merge` (ADR 0038).
//!
//! **Copied, not referenced.** [`create_project`](super::create::create_project)
//! writes each prompt into the project's own `agent_profiles` row. From that
//! moment the text is the project's: editing a profile edits nothing else, and
//! upgrading Mars changes no existing project's agents. The templates here are
//! only what a *new* project starts from.
//!
//! The prompts live beside this file as `templates/<name>.md` and are embedded
//! with [`include_str!`], so they are one text with no escaping, reviewable as
//! prose in a diff. `SPEC.md`, "Role profile templates" reproduces all four
//! verbatim and `tests/profile_templates.rs` compares the two, the way
//! `tests/mcp_descriptions.rs` does for the tool descriptions.
//!
//! They name the seeded state names (`backlog`, `ready`, `review`, `merge`,
//! `done`) and the MCP tools of `SPEC.md`, "MCP tool contracts", and nothing
//! else: no product name of any task tracker, because the tracker an agent is
//! told to use is the one its session is connected to.
//!
//! The three roles that build or check code — implementer, reviewer, merger —
//! also carry one paragraph about the session container: it is disposable and
//! the agent's own, a missing toolchain is installed rather than reported as a
//! blocker, and there is no root, so an install is user-level and anything
//! needing root belongs in the image (`ARCHITECTURE.md`, "Session container
//! specification"). It names no image and no tool as present, because a
//! profile's image is editable; tools appear only as examples of how to
//! install.

use uuid::Uuid;

use crate::models::{NewAgentProfile, ProfileResult};
// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// One seeded role: everything about it that is not a documented default.
///
/// Kind, backend, permission mode, idle timeout, partial messages, model,
/// runtime and secrets are deliberately absent — a template says what makes
/// the role a role, and [`NewAgentProfile::new`] supplies the rest, so a
/// changed default reaches the seeded profiles without being repeated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileTemplate {
    /// The profile's name, which is also the role.
    pub name: &'static str,
    /// The queue states the role picks work up from.
    pub serves_states: &'static [&'static str],
    /// The profile-gated git tools the role needs; task tools are always
    /// served (`SPEC.md`, "MCP tool contracts").
    pub mcp_tools: &'static [&'static str],
    /// Whether this is the profile a project launches from unless told
    /// otherwise. Exactly one template has it.
    pub is_default: bool,
    /// The system prompt, appended to every launch of the profile.
    pub system_prompt: &'static str,
}

impl ProfileTemplate {
    /// This template as an insertable profile of `project_id` on `image`,
    /// validated.
    ///
    /// Through [`NewAgentProfile::new`] and then the template's own fields, so
    /// every rule of `SPEC.md`, "Agent profiles" — the name length, the known
    /// tool names, the 64 KiB prompt — is checked on a seeded profile exactly
    /// as it is on one a user posts.
    pub fn to_new_profile(&self, project_id: Uuid, image: &str) -> ProfileResult<NewAgentProfile> {
        let mut profile = NewAgentProfile::new(project_id, self.name, image)?;
        profile.system_prompt = Some(self.system_prompt.to_string());
        profile.mcp_tools = self.mcp_tools.iter().map(|t| (*t).to_string()).collect();
        profile.serves_states = self
            .serves_states
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        profile.is_default = self.is_default;
        profile.validate()?;

        Ok(profile)
    }
}

/// The four templates, in the order a task travels through them, which is the
/// order they are seeded and therefore listed in
/// (`SPEC.md`, "Role profile templates").
///
/// `push` is given to nobody: nothing in a session may reach the upstream
/// remote by itself (ADR 0007). `rebase` is given to nobody either. The
/// merger must not have it, because an approval is bound to the commit that
/// was reviewed (ADR 0018) and a rebase would make a commit nobody reviewed;
/// the implementer does not need it, because its work clone's `origin` is the
/// project repository and it can fetch and rebase there itself
/// (`ARCHITECTURE.md`, "Git model" → "Session clone").
pub fn profile_templates() -> &'static [ProfileTemplate] {
    const TEMPLATES: &[ProfileTemplate] = &[
        ProfileTemplate {
            name: "planner",
            serves_states: &["backlog"],
            mcp_tools: &[],
            is_default: false,
            system_prompt: include_str!("templates/planner.md"),
        },
        ProfileTemplate {
            name: "implementer",
            serves_states: &["ready"],
            mcp_tools: &[],
            is_default: true,
            system_prompt: include_str!("templates/implementer.md"),
        },
        ProfileTemplate {
            name: "reviewer",
            serves_states: &["review"],
            mcp_tools: &["list_session_branches"],
            is_default: false,
            system_prompt: include_str!("templates/reviewer.md"),
        },
        ProfileTemplate {
            name: "merger",
            serves_states: &["merge"],
            mcp_tools: &["list_session_branches", "merge"],
            is_default: false,
            system_prompt: include_str!("templates/merger.md"),
        },
    ];

    TEMPLATES
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_IMAGE: &str = "localhost/mars-session:test";

    /// A list of static names as owned strings, which is what a validated
    /// profile carries.
    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn every_template_is_a_valid_profile() {
        for template in profile_templates() {
            let profile = template
                .to_new_profile(Uuid::nil(), TEST_IMAGE)
                .unwrap_or_else(|e| panic!("`{}` is a valid profile: {e}", template.name));

            assert_eq!(profile.name, template.name);
            assert_eq!(profile.serves_states, owned(template.serves_states));
            assert_eq!(profile.mcp_tools, owned(template.mcp_tools));
            assert_eq!(profile.is_default, template.is_default);
            assert_eq!(
                profile.system_prompt.as_deref(),
                Some(template.system_prompt)
            );
        }
    }

    #[test]
    fn exactly_one_template_is_the_default() {
        let defaults: Vec<&str> = profile_templates()
            .iter()
            .filter(|t| t.is_default)
            .map(|t| t.name)
            .collect();

        assert_eq!(defaults, ["implementer"]);
    }

    #[test]
    fn the_templates_are_the_four_roles_in_board_order() {
        let roles: Vec<(&str, &[&str])> = profile_templates()
            .iter()
            .map(|t| (t.name, t.serves_states))
            .collect();

        assert_eq!(
            roles,
            [
                ("planner", &["backlog"][..]),
                ("implementer", &["ready"][..]),
                ("reviewer", &["review"][..]),
                ("merger", &["merge"][..]),
            ]
        );
    }

    #[test]
    fn no_template_asks_for_push_or_rebase() {
        for template in profile_templates() {
            for forbidden in ["push", "rebase"] {
                assert!(
                    !template.mcp_tools.contains(&forbidden),
                    "`{}` must not be given `{forbidden}`",
                    template.name,
                );
            }
        }
    }

    #[test]
    fn a_prompt_fits_the_column_limit() {
        for template in profile_templates() {
            assert!(
                template.system_prompt.len() <= crate::models::MAX_SYSTEM_PROMPT_BYTES,
                "`{}`'s prompt is too long",
                template.name,
            );
            assert!(
                !template.system_prompt.trim().is_empty(),
                "`{}` has no prompt",
                template.name,
            );
        }
    }
}
