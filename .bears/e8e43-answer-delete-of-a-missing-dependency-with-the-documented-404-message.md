---
id: e8e43
title: Answer DELETE of a missing dependency with the documented 404 message
status: done
priority: P3
created: "2026-09-19T13:44:13.360297134Z"
updated: "2026-09-19T17:49:52.702050827Z"
tags:
  - orchestrator
  - tracker
parent: "5h3y4"
attempts: 1
---

## Summary
`kctj2` asked for `DELETE /projects/{pid}/tasks/{id}/dependencies/{dep}?kind=` to answer 404 `dependency not found` when no edge of that kind exists. `Error::NotFound` carries no message in this crate, so `tracker::dependencies::remove_dependency` answers `{"status":404,"error":"not found"}`, which a client cannot tell apart from an unknown task or an unknown `{dep}`.

Found while reviewing `kctj2` during the work for `md2zq`.

## Documents
- `SPEC.md` "Tasks" (`DELETE .../dependencies/{dep}`), "REST API" (error body `{status, error}`).
- `ARCHITECTURE.md` "Orchestrator internals" (the `Error` enum contract).

## Acceptance criteria
- [ ] Decide between a message-carrying 404 variant on `Error` (documented in `ARCHITECTURE.md`, "Orchestrator internals", in the same commit) and keeping the generic body; if the generic body stays, record that in `SPEC.md` so the MCP `update` tool's `remove_depends_on` is written against it.
- [ ] If the variant is added: `remove_dependency` answers 404 `dependency not found`, the unknown-task and unknown-`{dep}` cases keep `not found`, and `tests/task_dependencies_api.rs` asserts the three bodies.