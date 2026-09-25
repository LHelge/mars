//! The role profiles a project is created with, and the ones it is only
//! offered.
//!
//! A role in Mars is not code: it is a profile's served states plus its system
//! prompt (`ARCHITECTURE.md`, "Task tracker" → "State is a queue"). A project
//! that started with one blank agent left every user to write those
//! prompts before anything could flow through the board, so creation seeds
//! them: a planner over `backlog`, an implementer over `ready` and a reviewer
//! over `review` (ADR 0038). The fourth queue state, `merge`, is the
//! orchestrator's: a new project seeds it as an auto-merge state, so approved
//! work is merged without an agent (ADR 0045).
//!
//! In front of the roles comes `claude`, the project's default: a
//! conversational profile that serves no queue and asks for no git tool, which
//! is what the launch form preselects when a person just wants a Claude Code
//! session in the project (ADR 0051). The implementer and the reviewer are
//! ephemeral and carry `auto_launch`, so a task put in `ready` travels to
//! `done` with nobody launching anything once the project has an agent
//! credential the dispatcher can use; without one the dispatcher skips them
//! with `no_credential` (`ARCHITECTURE.md`, "Unattended launches").
//!
//! **Seeded is not the same as offered.** [`profile_templates`] is what
//! `GET /profile-templates` serves, and [`seeded_profile_templates`] — the
//! ones carrying [`ProfileTemplate::seeded`] — is what project creation
//! writes. Two templates are offered and not seeded, for two different
//! reasons. The `merger` would serve a state no task waits in for an agent:
//! the seeded `merge` state merges by itself, and a project that turns
//! `auto_merge` off and wants an agent there creates one from the template
//! (ADR 0045). The `tech-debt-scanner` is the first scheduled template
//! (`ARCHITECTURE.md`, "Scheduled agents"), and a schedule spends money on a
//! cadence nobody asked for — it runs whether or not there is any work — so
//! turning one on is a person's decision and not a side effect of creating a
//! project (ADR 0038). Auto-launch is different in kind: it only ever starts
//! work somebody queued (ADR 0051).
//!
//! **Copied, not referenced.** [`create_project`](super::create::create_project)
//! writes each prompt into the project's own `agent_profiles` row. From that
//! moment the text is the project's: editing a profile edits nothing else, and
//! upgrading Mars changes no existing project's agents. The templates here are
//! only what a *new* project starts from.
//!
//! The prompts live beside this file as `templates/<name>.md` — and a
//! scheduled template's run prompt as `templates/<name>.schedule.md` — and are
//! embedded with [`include_str!`], so they are one text with no escaping,
//! reviewable as prose in a diff. `SPEC.md`, "Role profile templates"
//! reproduces every one of them verbatim and `tests/profile_templates.rs`
//! compares the two, the way `tests/mcp_descriptions.rs` does for the tool
//! descriptions.
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

use crate::models::{NewAgentProfile, ProfileKind, ProfileResult};
// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// One offered role: everything about it that is not a documented default.
///
/// Backend, permission mode, idle timeout, partial messages, model, runtime
/// and secrets are deliberately absent — a template says what makes the role a
/// role, and [`NewAgentProfile::new`] supplies the rest, so a changed default
/// reaches the profiles created from a template without being repeated here.
/// `kind` is here because a scheduled role has to be `ephemeral` to carry a
/// schedule at all (`SPEC.md`, "Agent profiles" → "Scheduled profiles").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileTemplate {
    /// The profile's name, which is also the role.
    pub name: &'static str,
    /// Whether the role talks to a person or runs one prompt and ends.
    pub kind: ProfileKind,
    /// The queue states the role picks work up from.
    pub serves_states: &'static [&'static str],
    /// The profile-gated git tools the role needs; task tools are always
    /// served (`SPEC.md`, "MCP tool contracts").
    pub mcp_tools: &'static [&'static str],
    /// Whether this is the profile a project launches from unless told
    /// otherwise. Exactly one template has it: `claude`.
    pub is_default: bool,
    /// Whether the dispatcher may launch this role by itself (`SPEC.md`,
    /// "Agent profiles"; ADR 0042). Only an ephemeral role can carry it, and
    /// the seeded implementer and reviewer do (ADR 0051).
    pub auto_launch: bool,
    /// Whether project creation writes this role into a new project. `claude`
    /// and the three roles a task travels through do; the merger, whose state
    /// the orchestrator serves, and a scheduled role are offered only (ADR
    /// 0038, 0045, 0051).
    pub seeded: bool,
    /// The system prompt, appended to every launch of the profile.
    pub system_prompt: &'static str,
    /// The UTC 5-field cron expression a scheduled role runs on, or `None` for
    /// a role a person launches.
    pub schedule_cron: Option<&'static str>,
    /// The message each scheduled run is given; set exactly when
    /// [`ProfileTemplate::schedule_cron`] is.
    pub schedule_prompt: Option<&'static str>,
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
        profile.kind = self.kind;
        // `NewAgentProfile::new` validated a conversational profile and so
        // resolved this to that kind's default; a template never states it, so
        // the kind set just above is what decides (`SPEC.md`, "Agent
        // profiles").
        profile.partial_messages = None;
        profile.schedule_cron = self.schedule_cron.map(str::to_string);
        profile.schedule_prompt = self.schedule_prompt.map(str::to_string);
        profile.system_prompt = Some(self.system_prompt.to_string());
        profile.mcp_tools = self.mcp_tools.iter().map(|t| (*t).to_string()).collect();
        profile.serves_states = self
            .serves_states
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        profile.is_default = self.is_default;
        profile.auto_launch = self.auto_launch;
        profile.validate()?;

        Ok(profile)
    }
}

/// Every template, in the order of the table of `SPEC.md`, "Role profile
/// templates": `claude`, the default, first; then the four queue roles, in the
/// order a task travels through them; then the scheduled role. That is also
/// the order the seeded ones are written in, so `claude` is the first row
/// `GET /projects/{pid}/profiles` of a new project returns.
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
        // The default (ADR 0051): a plain conversational session that serves
        // no queue — serving `ready` would show it the implementer's work —
        // and asks for no git tool. The task tools are served to it anyway.
        ProfileTemplate {
            name: "claude",
            kind: ProfileKind::Conversational,
            serves_states: &[],
            mcp_tools: &[],
            is_default: true,
            auto_launch: false,
            seeded: true,
            system_prompt: include_str!("templates/claude.md"),
            schedule_cron: None,
            schedule_prompt: None,
        },
        ProfileTemplate {
            name: "planner",
            kind: ProfileKind::Conversational,
            serves_states: &["backlog"],
            mcp_tools: &[],
            is_default: false,
            auto_launch: false,
            seeded: true,
            system_prompt: include_str!("templates/planner.md"),
            schedule_cron: None,
            schedule_prompt: None,
        },
        ProfileTemplate {
            name: "implementer",
            // Ephemeral and auto-launched, like the reviewer (ADR 0051): the
            // dispatcher puts it on each task in `ready` once an agent
            // credential it can use is stored.
            kind: ProfileKind::Ephemeral,
            serves_states: &["ready"],
            mcp_tools: &[],
            is_default: false,
            auto_launch: true,
            seeded: true,
            system_prompt: include_str!("templates/implementer.md"),
            schedule_cron: None,
            schedule_prompt: None,
        },
        ProfileTemplate {
            name: "reviewer",
            kind: ProfileKind::Ephemeral,
            serves_states: &["review"],
            mcp_tools: &["list_session_branches"],
            is_default: false,
            auto_launch: true,
            seeded: true,
            system_prompt: include_str!("templates/reviewer.md"),
            schedule_cron: None,
            schedule_prompt: None,
        },
        ProfileTemplate {
            name: "merger",
            kind: ProfileKind::Conversational,
            serves_states: &["merge"],
            mcp_tools: &["list_session_branches", "merge"],
            is_default: false,
            auto_launch: false,
            // The seeded `merge` state is an auto-merge state (ADR 0045):
            // offered for a project that turns that off, never seeded.
            seeded: false,
            system_prompt: include_str!("templates/merger.md"),
            schedule_cron: None,
            schedule_prompt: None,
        },
        // The first scheduled template (`ARCHITECTURE.md`, "Scheduled
        // agents"). Ephemeral because only an ephemeral profile may carry a
        // schedule; `ready` as its served state because `ready` — the one tool
        // that lists tasks — lists the calling profile's served states alone,
        // and a scanner that cannot see what is already queued would file the
        // same task every day; no git tool because filing a task needs none.
        ProfileTemplate {
            name: "tech-debt-scanner",
            kind: ProfileKind::Ephemeral,
            serves_states: &["ready"],
            mcp_tools: &[],
            is_default: false,
            auto_launch: false,
            seeded: false,
            system_prompt: include_str!("templates/tech-debt-scanner.md"),
            schedule_cron: Some("0 4 * * *"),
            schedule_prompt: Some(include_str!("templates/tech-debt-scanner.schedule.md")),
        },
    ];

    TEMPLATES
}

/// The templates project creation seeds, in the order it writes them:
/// `claude`, the planner, the implementer and the reviewer — not the merger,
/// whose `merge` state the orchestrator serves (ADR 0045), and nothing on a
/// schedule. The implementer and the reviewer auto-launch, which spends money
/// only on work somebody queued and only once an agent credential the
/// dispatcher can use is stored (ADR 0051; `SPEC.md`, "Role profile
/// templates").
pub fn seeded_profile_templates() -> impl Iterator<Item = &'static ProfileTemplate> {
    profile_templates()
        .iter()
        .filter(|template| template.seeded)
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
            assert_eq!(profile.kind, template.kind);
            assert_eq!(profile.serves_states, owned(template.serves_states));
            assert_eq!(profile.mcp_tools, owned(template.mcp_tools));
            assert_eq!(profile.is_default, template.is_default);
            assert_eq!(profile.auto_launch, template.auto_launch);
            assert_eq!(
                profile.system_prompt.as_deref(),
                Some(template.system_prompt)
            );
            assert_eq!(profile.schedule_cron.as_deref(), template.schedule_cron);
            assert_eq!(profile.schedule_prompt.as_deref(), template.schedule_prompt);
            // Resolved from the template's own kind, not from the
            // conversational default `NewAgentProfile::new` starts at.
            assert_eq!(
                profile.partial_messages,
                Some(template.kind.default_partial_messages()),
                "`{}`: partial messages follow the kind",
                template.name,
            );
        }
    }

    #[test]
    fn claude_and_the_three_queue_roles_are_seeded() {
        let seeded: Vec<&str> = seeded_profile_templates().map(|t| t.name).collect();

        assert_eq!(seeded, ["claude", "planner", "implementer", "reviewer"]);
    }

    #[test]
    fn auto_launch_is_only_on_ephemeral_templates() {
        for template in profile_templates() {
            if template.auto_launch {
                assert_eq!(
                    template.kind,
                    ProfileKind::Ephemeral,
                    "`{}` auto-launches and must be ephemeral",
                    template.name,
                );
            }
        }
        let auto: Vec<&str> = profile_templates()
            .iter()
            .filter(|t| t.auto_launch)
            .map(|t| t.name)
            .collect();

        assert_eq!(auto, ["implementer", "reviewer"]);
    }

    #[test]
    fn the_default_serves_no_queue_and_talks_to_a_person() {
        let default = profile_templates()
            .iter()
            .find(|t| t.is_default)
            .expect("one template is the default");

        assert_eq!(default.kind, ProfileKind::Conversational);
        assert!(default.serves_states.is_empty());
        assert!(default.mcp_tools.is_empty());
        assert!(!default.auto_launch);
        assert!(default.seeded);
    }

    #[test]
    fn no_seeded_template_carries_a_schedule() {
        // A schedule spends money on a cadence nobody asked for, so it is
        // never a side effect of creating a project.
        for template in seeded_profile_templates() {
            assert_eq!(
                template.schedule_cron, None,
                "`{}` is seeded and scheduled",
                template.name,
            );
        }
    }

    #[test]
    fn a_scheduled_template_is_ephemeral_and_carries_its_run_prompt() {
        for template in profile_templates() {
            assert_eq!(
                template.schedule_cron.is_some(),
                template.schedule_prompt.is_some(),
                "`{}`: the two schedule fields stand or fall together",
                template.name,
            );
            if template.schedule_cron.is_some() {
                assert_eq!(
                    template.kind,
                    ProfileKind::Ephemeral,
                    "`{}` is scheduled and must be ephemeral",
                    template.name,
                );
                assert!(
                    !template
                        .schedule_prompt
                        .expect("a scheduled template has a run prompt")
                        .trim()
                        .is_empty(),
                    "`{}` has a blank run prompt",
                    template.name,
                );
            }
        }
    }

    #[test]
    fn exactly_one_template_is_the_default() {
        let defaults: Vec<&str> = profile_templates()
            .iter()
            .filter(|t| t.is_default)
            .map(|t| t.name)
            .collect();

        assert_eq!(defaults, ["claude"]);
    }

    #[test]
    fn the_templates_are_the_default_the_four_roles_in_board_order_and_the_scanner() {
        let roles: Vec<(&str, &[&str])> = profile_templates()
            .iter()
            .map(|t| (t.name, t.serves_states))
            .collect();

        assert_eq!(
            roles,
            [
                ("claude", &[][..]),
                ("planner", &["backlog"][..]),
                ("implementer", &["ready"][..]),
                ("reviewer", &["review"][..]),
                ("merger", &["merge"][..]),
                ("tech-debt-scanner", &["ready"][..]),
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
