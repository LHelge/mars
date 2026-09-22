---
id: k3vrx
title: "Help topic: how a session is instructed (system prompt, task message, launch message, schedule prompt)"
status: done
priority: P2
created: "2026-09-22T21:14:32.718525124Z"
updated: "2026-09-22T21:48:34.326188250Z"
tags:
  - frontend
  - docs
depends_on:
  - v9rjg
parent: gtbp5
attempts: 1
---

Users can't tell how the profile's system prompt, the launch message, a task and a schedule prompt combine. Add a help topic `instructions` ("How a session is instructed") to `HELP_TOPICS`/`HELP_TOPIC_TITLES` (after `profiles`) with `src/help/instructions.md`, and link it from the fields that feed it. Invoke `/frontend-design` first.

Facts to convey (verified in code at a8854a6; re-check before writing):
- **Standing instructions, every launch of the profile:** the CLI's own system prompt; the profile's `system_prompt` appended with `--append-system-prompt`, re-read on every launch including resume (`--system-prompt-snapshot off`), so editing a profile changes its parked sessions on their next wake (ARCHITECTURE "Agent process model"); the repository's `CLAUDE.md`, `.mcp.json` and skills from the checkout; the Mars MCP server's fixed instructions (`orchestrator/src/mcp/server.rs` `INSTRUCTIONS`: Mars is the task tracker for this session).
- **First message, by launch path** (`session/create.rs` `first_message`, `session/task_message.rs` `generated_task_message`, `cron/dispatcher.rs`, `cron/schedules.rs`):
  - by a user, no task: the user's message if any; a conversational session without one waits; an ephemeral one requires it.
  - by a user, for a task: the generated task message first ("You hold task #N: title. Call get_task…", plus the current hand-off's id, source session, branch, commit, review status and comment, plus a note when an explicit base ref overrides the hand-off), then the user's message.
  - dispatcher: the generated task message only.
  - schedule: `schedule_prompt` as the message, no task, default branch as base.
- **Delivery:** ephemeral — joined with a blank line into the single `-p` prompt; conversational — queued as separate user messages, the generated one with no author (shown as such in the transcript).
- **Guidance:** system prompt = who the agent is and how it hands work on; message / schedule prompt = what this run does; task details live in the task (the agent reads them with `get_task`), so don't paste the task into the message.

Links: ProfileEditor system prompt and schedule prompt fields; the message field of `pages/project/LaunchSessionForm.tsx` and `tasks/LaunchForTask.tsx`; cross-link from `profiles.md` "System prompt" and `getting-started.md` step 3 (`help:instructions`). Update `SPEC.md` "Frontend" → Help (the topic list), and add a short consolidated paragraph to `ARCHITECTURE.md` (e.g. under "Agent process model") if no single place states the assembly order today. Update `tests/help.spec.ts` / content tests if they enumerate topics.