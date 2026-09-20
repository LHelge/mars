//! The twelve tool names, in listing order, with their classification and
//! their description (`SPEC.md`, "MCP tool contracts").
//!
//! One enum rather than string literals scattered over the dispatcher: a tool
//! is named in the listing, in the profile gate and in the dispatch `match`,
//! and a typo in any one of the three is a tool that silently disappears or
//! answers `not found`. [`ToolName::parse`] is the single place a name from
//! the wire becomes a tool, and [`ToolName::ALL`] is the single listing order.
//!
//! The order is the document's own: the eight task tools first, then the four
//! git tools. Task tools are always allowed; git tools are profile-gated
//! ([`ToolName::is_git`]).

use super::descriptions;

/// One of the twelve tools `SPEC.md` defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolName {
    /// `ready`: the claimable tasks in the profile's served states.
    Ready,
    /// `claim`: take a task's lease.
    Claim,
    /// `get_task`: one task in full.
    GetTask,
    /// `update`: change a task the caller holds, optionally handing off.
    Update,
    /// `release`: give a held task back.
    Release,
    /// `comment`: leave a comment on any task in the project.
    Comment,
    /// `needs_human`: escalate a task to the project's human state.
    NeedsHuman,
    /// `create_task`: file discovered work.
    CreateTask,
    /// `list_session_branches`: the mirror's session branches (git).
    ListSessionBranches,
    /// `merge`: merge into an integration branch (git).
    Merge,
    /// `rebase`: rebase a branch onto another (git).
    Rebase,
    /// `push`: publish a mirror branch upstream (git).
    Push,
}

impl ToolName {
    /// Every tool, in the order `SPEC.md` lists them and therefore the order
    /// `tools/list` answers with.
    pub const ALL: [ToolName; 12] = [
        ToolName::Ready,
        ToolName::Claim,
        ToolName::GetTask,
        ToolName::Update,
        ToolName::Release,
        ToolName::Comment,
        ToolName::NeedsHuman,
        ToolName::CreateTask,
        ToolName::ListSessionBranches,
        ToolName::Merge,
        ToolName::Rebase,
        ToolName::Push,
    ];

    /// The name on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ToolName::Ready => "ready",
            ToolName::Claim => "claim",
            ToolName::GetTask => "get_task",
            ToolName::Update => "update",
            ToolName::Release => "release",
            ToolName::Comment => "comment",
            ToolName::NeedsHuman => "needs_human",
            ToolName::CreateTask => "create_task",
            ToolName::ListSessionBranches => "list_session_branches",
            ToolName::Merge => "merge",
            ToolName::Rebase => "rebase",
            ToolName::Push => "push",
        }
    }

    /// A name from a `tools/call` request, or `None` for a tool that does not
    /// exist.
    pub fn parse(name: &str) -> Option<ToolName> {
        ToolName::ALL.into_iter().find(|tool| tool.as_str() == name)
    }

    /// Is this one of the four profile-gated git tools?
    ///
    /// The task tools are always allowed; a git tool is listed and dispatched
    /// only when the calling session's profile names it in `mcp_tools`
    /// (`SPEC.md`, "MCP tool contracts").
    pub fn is_git(self) -> bool {
        matches!(
            self,
            ToolName::ListSessionBranches | ToolName::Merge | ToolName::Rebase | ToolName::Push
        )
    }

    /// The verbatim description from `SPEC.md`, which `tools/list` sends.
    pub fn description(self) -> &'static str {
        match self {
            ToolName::Ready => descriptions::READY,
            ToolName::Claim => descriptions::CLAIM,
            ToolName::GetTask => descriptions::GET_TASK,
            ToolName::Update => descriptions::UPDATE,
            ToolName::Release => descriptions::RELEASE,
            ToolName::Comment => descriptions::COMMENT,
            ToolName::NeedsHuman => descriptions::NEEDS_HUMAN,
            ToolName::CreateTask => descriptions::CREATE_TASK,
            ToolName::ListSessionBranches => descriptions::LIST_SESSION_BRANCHES,
            ToolName::Merge => descriptions::MERGE,
            ToolName::Rebase => descriptions::REBASE,
            ToolName::Push => descriptions::PUSH,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_twelve_names_are_the_documented_ones_in_the_documented_order() {
        assert_eq!(
            ToolName::ALL.map(ToolName::as_str),
            [
                "ready",
                "claim",
                "get_task",
                "update",
                "release",
                "comment",
                "needs_human",
                "create_task",
                "list_session_branches",
                "merge",
                "rebase",
                "push",
            ],
        );
    }

    #[test]
    fn every_name_round_trips_through_parse() {
        for tool in ToolName::ALL {
            assert_eq!(ToolName::parse(tool.as_str()), Some(tool));
        }
    }

    #[test]
    fn an_unknown_or_misspelt_name_parses_to_nothing() {
        for name in ["", "Ready", "get-task", "list_branches", "delete_task"] {
            assert_eq!(ToolName::parse(name), None, "{name}");
        }
    }

    #[test]
    fn the_last_four_are_the_git_tools_and_the_first_eight_are_not() {
        let git: Vec<_> = ToolName::ALL
            .into_iter()
            .filter(|tool| tool.is_git())
            .map(ToolName::as_str)
            .collect();

        assert_eq!(git, ["list_session_branches", "merge", "rebase", "push"]);
    }

    #[test]
    fn every_tool_has_a_non_empty_description_of_its_own() {
        let mut descriptions: Vec<_> = ToolName::ALL
            .into_iter()
            .map(|tool| {
                let description = tool.description();
                assert!(!description.is_empty(), "{}", tool.as_str());
                assert!(!description.contains('\n'), "{}", tool.as_str());
                description
            })
            .collect();
        descriptions.sort_unstable();
        descriptions.dedup();

        assert_eq!(descriptions.len(), 12);
    }
}
