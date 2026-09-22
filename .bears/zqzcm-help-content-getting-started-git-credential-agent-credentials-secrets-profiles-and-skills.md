---
id: zqzcm
title: "Help content: getting started, git credential, agent credentials, secrets, profiles and skills"
status: done
priority: P2
created: "2026-09-22T19:08:28.053474886Z"
updated: "2026-09-22T21:22:54.771929235Z"
tags:
  - frontend
  - docs
depends_on:
  - ujccg
parent: gtbp5
attempts: 1
---

Write the Markdown for these `src/help/` topics. Plain, task-oriented prose for a user of the console; every claim must agree with the source docs (cite nothing to the user, but check against them). Where a doc turns out wrong, fix the doc in the same commit.

- `getting-started`: create a project → add an agent credential → launch from a starter profile; the four starter roles. Source: README "Start" (last paragraphs), SPEC "User-facing features".
- `git-credential`: what the token is used for (fetch and push, orchestrator only, never in a session), the host/permission table (GitHub fine-grained Contents R/W + Workflows R/W for `.github/workflows`; classic `repo`/`workflow`; GitLab `read_repository`/`write_repository`), read-only token = pushes fail, public repo needs one before first push, rotating = replace the value of the project's `GIT_CREDENTIAL` on Secrets, keep it orchestrator-only and do not rename it. Source: README "Repository credentials".
- `agent-credentials`: subscription token vs API key, one per scope, most specific scope wins (me → project → everyone), unattended launches (dispatcher, schedules) need a project or Everyone credential, an expired token parks the session with an error naming the secret — replace it and send the next message. Source: SPEC "Agent credentials" (features + Secrets), ARCHITECTURE "Secrets" → Agent credentials, ADR 0036.
- `secrets`: a secret reaches a session only when a profile declares its name; user overrides project overrides global; orchestrator-only is for Mars itself and is never injected; names are env-var names; transcripts are stored unredacted and can contain secrets an agent printed (ADR 0027). Source: ARCHITECTURE "Secrets" → Resolution at launch, SPEC "Secrets".
- `profiles`: conversational vs ephemeral (parking, resume, no retry for ephemeral, only ephemeral can be automated), served states, model, image (built FROM the session base, no system packages at runtime, base vs -dev), runtime (must be registered with the engine), idle timeout (park vs stalled; silence counts), git tools with what each allows (`push` publishes with the project's git credential), system prompt describes the job while the repo's CLAUDE.md/.mcp.json/skills describe conventions. Source: SPEC "Agent profiles", "Role profile templates", "MCP tool contracts"; ARCHITECTURE "Agent process model", "Session image".
- `skills`: repo `.claude/skills/` vs the project CLI state directory, `/session/home/.claude` is not read, picked up on next session/resume, not shown in the UI. Source: README "Skills and plugins".