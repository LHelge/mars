---
id: ectnt
title: Document installing skills and the repository credential's required permissions
status: done
priority: P3
created: "2026-09-22T19:04:05.542459879Z"
updated: "2026-09-22T19:04:09.972987744Z"
tags:
  - docs
---

README.md, "Operating notes": new "Repository credentials" (which PAT permissions GitHub/GitLab need, since the orchestrator fetches and pushes with `GIT_CREDENTIAL`; ARCHITECTURE.md "Git" → Credentials) and "Skills and plugins" (repo `.claude/skills/` vs the per-project `CLAUDE_CONFIG_DIR`, ADR 0015; `/session/home/.claude` is not read). Prerequisites line about the PAT points there.