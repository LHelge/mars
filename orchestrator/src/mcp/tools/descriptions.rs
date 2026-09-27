//! The thirteen tool descriptions, copied verbatim from `SPEC.md`, "MCP tool
//! contracts".
//!
//! "Tool descriptions are part of the contract because they steer the agent.
//! They are reproduced verbatim." This module is the only place the text lives
//! in code: [`ToolName::description`](super::ToolName::description) reads it,
//! and `tests/mcp_descriptions.rs` parses the document and fails if a single
//! byte drifts apart. When the two disagree the document wins.
//!
//! Every constant is one line, however long: a wrapped literal would introduce
//! newlines the document does not have.

/// `ready`.
pub const READY: &str = "List tasks you could start now, in the states your profile serves. Claim one with `claim` before doing any work on it. If you were launched for a task you already hold it: call `get_task` on it instead.";

/// `claim`.
pub const CLAIM: &str = "Claim a task before you start it. You hold it until you hand it off with `update`, give it back with `release`, or your session ends. If the claim fails someone else has it: pick another.";

/// `get_task`.
pub const GET_TASK: &str = "Read a task in full: description, comments, dependencies, sub-tasks and which sessions worked on it. Read it before you start; the comments are where earlier agents and humans left context for you.";

/// `update`.
pub const UPDATE: &str = "Update a task you hold. Changing to a different `state` ends your hold. When handing over code, commit it first and include a revision hand-off with the exact commit and a comment describing the work and checks. When reviewing, forward the existing hand-off with your decision; do not substitute your own branch.";

/// `release`.
pub const RELEASE: &str = "Give a task back without finishing it, and say why. Use this when you cannot make progress. The task stays in its state for another agent; after too many attempts it goes to a human instead.";

/// `comment`.
pub const COMMENT: &str = "Leave a comment on a task. This is how you talk to other agents and to humans: say what you found, what you changed, what you need. Comment before you hand off.";

/// `needs_human`.
pub const NEEDS_HUMAN: &str = "Hand a task to a human when you are blocked on a decision, credentials, or anything you must not decide alone. Say exactly what you need.";

/// `create_task`.
pub const CREATE_TASK: &str = "Create a task when you discover work outside what you hold: a follow-up, a bug, or a sub-task of a plan. Put it in the state that matches how ready it is (`backlog` if it still needs planning). Link it with `depends_on` if it must wait, and with `parent` if it is part of a larger task. If you hold several tasks, identify the originating task with `discovered_from`.";

/// `create_plan`.
pub const CREATE_PLAN: &str = "Create several related tasks in one step: the sub-tasks of a plan, under a new parent (`new_parent`) or an existing task (`parent`), with the dependencies between them. Give each sub-task a short `ref` and list it in another sub-task's `depends_on` to make that one wait; `depends_on` also takes existing tasks. Everything is created together or nothing is, so no sub-task can be started before its prerequisites exist. Use this instead of several `create_task` calls whenever the tasks depend on each other.";

/// `list_session_branches`.
pub const LIST_SESSION_BRANCHES: &str = "List session branches in the project mirror with how far ahead/behind they are relative to the default branch.";

/// `merge`.
pub const MERGE: &str = "Merge into an integration branch. For reviewed task work, pass task_id and handoff_id to merge the exact approved commit. For an explicit generic branch merge, pass source. Fails with conflicting paths; never resolves conflicts for you.";

/// `rebase`.
pub const REBASE: &str = "Rebase a branch onto another in the mirror. Use it to bring a session branch up to date with the default branch before merging.";

/// `push`.
pub const PUSH: &str = "Push a mirror branch to the upstream remote. Only do this when a human or the task explicitly asks for it.";
