---
id: gtbp5
title: "In-app help: a Help page and pointers from the forms that need them"
type: epic
status: open
priority: P2
created: "2026-09-22T19:07:55.661019081Z"
updated: "2026-09-22T19:07:55.661019081Z"
tags:
  - frontend
  - docs
---

First-time operators hit concepts the UI never explains: which permissions the project's git token needs (Mars pushes with it), why unattended launches cannot use a "Me" agent credential, that a secret is injected only when a profile declares it, what max attempts does, integration heads vs upstream vs session refs, where skills come from. The answers exist in README.md, SPEC.md and ARCHITECTURE.md; nothing in the UI reaches them.

Shape:
- A `/help` page, one section per topic with a stable anchor, content bundled with the frontend (it is user-facing prose, not the operator docs verbatim, and must agree with them).
- Short, accurate field hints where a one-liner is enough, and a "Learn more" link to the matching help section where it is not. The link is its own prop, not inside the `hint` string, so `aria-describedby` keeps reading only the hint.

The audit that motivates each task is summarised in the task bodies (file:line references as of 3b4f17b).