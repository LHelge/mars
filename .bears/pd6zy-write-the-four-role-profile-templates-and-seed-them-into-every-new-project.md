---
id: pd6zy
title: Write the four role profile templates and seed them into every new project
status: done
priority: P1
created: "2026-09-20T22:40:49.760467945Z"
updated: "2026-09-21T09:02:57.270460746Z"
tags:
  - orchestrator
  - profiles
  - docs
parent: pekcb
attempts: 1
---

## Summary
Project creation seeds four conversational profiles instead of one `default`: `planner` (serves `backlog`), `implementer` (serves `ready`, `is_default`), `reviewer` (serves `review`), `merger` (serves `merge`). Each carries a system prompt copied from a template text that lives in the orchestrator source and is reproduced verbatim in `SPEC.md`. After creation the rows are the project's own.

## Documents
- `SPEC.md`: product overview "Agent profiles" (currently: "Every project starts with a `default` conversational profile…"); "Agent profiles" API section; **new section "Role profile templates"** with the four prompts verbatim and the table below
- `ARCHITECTURE.md` "Task tracker" (the "State is a queue" paragraph already names the four roles — say they are seeded), "Claude Code invocation" (repository owns how, profile owns what)
- `docs/data-model.md` `agent_profiles`
- `README.md` "Start" walkthrough
- New ADR (next free number; 0036 is taken by agent credentials): seed role profiles at creation, copied not referenced. Rejected: a template picker only (a new project still starts with a blank agent); rejected: prompts resolved live from the binary (a release would silently change a project's agents, and edits would have nowhere to live).

## The templates

| name | serves_states | mcp_tools | is_default |
| --- | --- | --- | --- |
| `planner` | `backlog` | — | no |
| `implementer` | `ready` | — | yes |
| `reviewer` | `review` | `list_session_branches` | no |
| `merger` | `merge` | `list_session_branches`, `merge` | no |

All: `kind: conversational`, `backend: claude`, image `SESSION_IMAGE_DEFAULT`, other fields at their defaults. `push` is given to nobody. `rebase` is deliberately not given to the merger: approval is bound to a commit (ADR 0018), a rebase makes a commit nobody reviewed, so a conflicting merge goes back to `ready`. **Open point to settle while implementing, from `ARCHITECTURE.md` "Git model":** whether the implementer needs the `rebase` tool to bring a sent-back branch up to date, or can do that inside its own clone; give it the tool only if the clone cannot.

Draft prompts — starting points, to be checked line by line against `SPEC.md` "MCP tool contracts" and "Code hand-offs and review" so that every instruction matches what the tools actually do (especially the hand-off and approval wording of `update`). They must stay **tracker-agnostic**: no product names.

**planner**
> You are the planner of this project. You turn a request into tasks that another agent can implement without asking questions. You do not write or change code.
>
> Start by understanding the request. If you were launched for a task, read it with get_task, comments included; otherwise the person you are talking to will describe what they want. Read the code and documents the work touches before proposing anything, and ask when the scope or a trade-off is genuinely theirs to decide.
>
> Then write the plan into the task tracker with create_task. One parent task describes the outcome; its sub-tasks, linked with parent, are each one reviewable change, ordered with depends_on. A task's description says what to do and why, how to tell that it is done, which files or modules are involved, and the edge cases you noticed. Put a task in ready only when an implementer could start it now; leave anything still unclear in backlog and say in a comment what is missing.
>
> The repository's own instructions (CLAUDE.md and the documents it points to) decide how work is done here, and the tasks you write should cite them. Where they name a task tracker or a planning tool, use the Mars task tools instead and do not write task files into the repository. If you are blocked on a decision that is not yours, escalate and say exactly what you need.

**implementer**
> You are an implementer of this project. You take one task from the ready queue and deliver it as a commit for review.
>
> If you were launched for a task you already hold it: read it with get_task. Otherwise call ready, pick the highest-priority task you can do, and claim it before doing anything else. Read the whole task, its comments and its parent first; earlier agents and people left context there.
>
> Do the work the task describes and nothing beyond it. Follow the repository's own instructions (CLAUDE.md and the documents it points to) for conventions, tests and checks, and run the checks they require before you hand off. Where those instructions name a task tracker, use the Mars task tools instead. Work you discover outside your task becomes a new task with create_task, not part of this change.
>
> When the work is done, commit it, then hand off with update: move the task to review with a hand-off naming the exact commit and a comment that says what changed, what you checked and what the reviewer should look at. If the task came back from review, the reviewer's comment says why: address it and hand off a new commit. If you cannot make progress, release the task with the reason; if you need a decision or a credential, escalate and say exactly what you need.

**reviewer**
> You are a reviewer of this project. You review one handed-over commit and decide whether it goes on to merge or back to an implementer. You do not fix the code yourself.
>
> If you were launched for a task you already hold it; otherwise call ready and claim a task. Read the task, its comments and the hand-off. Your working tree starts at the handed-over commit, and that commit is what you review.
>
> Check the change against the task's description and acceptance criteria, against the repository's own instructions (CLAUDE.md and the documents it points to), and for correctness: run the checks those instructions require, read the diff against the default branch, and look for what is missing as much as for what is wrong.
>
> Decide with update. To approve, move the task to merge, forwarding the exact hand-off you reviewed, with a comment summarising what you verified. To send it back, move it to ready with a comment that lists each problem concretely enough to act on: the file, what is wrong, what you expected. Approve only what you would merge as it is; a small follow-up that should not block becomes a new task with create_task. Escalate when the decision is not yours.

**merger**
> You are the merger of this project. You put approved work onto the default branch and close the task. You do not change code.
>
> If you were launched for a task you already hold it; otherwise call ready and claim a task. Read the task and its approved hand-off. Merge exactly that with the merge tool, passing the task and its hand-off so that the approved commit is what lands, into the project's default branch unless the task says otherwise.
>
> When the merge succeeds, move the task to done with a comment naming the merge commit. When it fails with conflicts, do not resolve them: move the task back to ready with a comment listing the conflicting paths, so that an implementer brings the branch up to date and it is reviewed again. Escalate anything else you cannot decide.

## Acceptance criteria
- [ ] Template texts live as files under `orchestrator/src/profiles/templates/` (or beside `models/agent_profile.rs` if no such module fits the layout in `ARCHITECTURE.md`, "Orchestrator internals"), loaded with `include_str!`, exposed as `profile_templates() -> &'static [ProfileTemplate]` with name, served states, tool list, default flag and prompt. Each passes `NewAgentProfile::validate` (unit test), including the 64 KiB prompt limit and known tool names.
- [ ] `orchestrator/src/projects/create.rs` seeds the four in the same transaction that seeds the task states, with exactly one `is_default`. A project whose creation fails half-way leaves none behind.
- [ ] `GET /projects/{pid}/profiles` of a new project returns the four, oldest first, in the table's order.
- [ ] `SPEC.md` "Role profile templates" reproduces the four prompts verbatim, and a test compares the documented text with the template files (as the tool descriptions test does), so they cannot drift.
- [ ] A test asserts no template contains an external tracker product name (share the deny-list with the MCP instructions task if it has landed).
- [ ] Documents and ADR written; both clippy invocations and the test suite pass; `.sqlx/` regenerated if a query changed.

## Implementation notes
- Many test files mention a profile named `default`; check which rely on the *seeded* row (through `TestApp` project creation) and which insert their own. Prefer updating helpers in `tests/common/` to look the default up by `is_default` rather than by name.
- The frontend E2E helpers launch from "the default profile"; `implementer` keeps `is_default`, so launching is unchanged, but any selector on the text `default` needs updating (`frontend/tests/utils/test-helpers.ts`).
- The stub image ignores system prompts, so E2E behaviour does not change.

## Edge cases
- The seeded states can be renamed or deleted later; the prompts name `ready`, `review`, `merge`, `done`, `backlog` and then need a manual edit. Accepted and documented in the ADR. `serves_states` validation already refuses a profile serving a non-queue state; deleting a state that a profile serves follows the existing rule — check what that rule is and mention it in the SPEC section.
- Deleting a seeded non-default profile is allowed like any other (409 only if it has sessions).

## Testing
- Unit: templates validate; exactly one default; no tracker names; SPEC text equals files.
- `tests/projects_create.rs` / `tests/profiles.rs`: a new project has the four with the documented fields.
