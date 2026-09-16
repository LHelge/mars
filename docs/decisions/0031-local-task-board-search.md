# 0031. Filter the task board locally by title or number

Status: accepted.

## Context

A growing board needs a quick way to find a task. v1 already loads the complete project task list, so server-side search would add an API contract without improving this use case.

## Decision

Add a task-board search field for case-insensitive title substrings and exact task numbers, with or without a leading `#`. Filter locally across all state columns using the board's existing snapshot. Keep the snapshot intact and reapply the query after live refreshes. Clearing the query restores all cards; changing projects clears it.

Keep the existing column/task order, distinguish no matches from loading or errors, and preserve access to an open task drawer or direct task URL. Exact matching and empty-state behavior are specified in `SPEC.md`, "Task-board search".

## Consequences

No endpoint, schema change or search service is required. Search covers titles and task numbers, not descriptions or comments. If task-list pagination is introduced later, this complete-project search contract must be revisited.

Acceptance covers title case differences, whitespace, exact numbers versus number prefixes, clearing, no matches, live updates while filtered, project switching and direct task navigation.
