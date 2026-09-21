---
id: qh8ue
title: "Keep feature UI out of the entry chunk: stop importing the components barrel on the first-paint path; delete dead exports"
status: done
priority: P2
created: "2026-09-21T10:56:13.188192720Z"
updated: "2026-09-21T23:08:47.595388509Z"
tags:
  - frontend
  - technical-review
  - build
  - refactor
parent: "579dz"
attempts: 1
---

Problem: src/AuthBootstrap.tsx:27 imports Alert, LoadingState and SubmitButton from "./components". That barrel re-exports SecretsManager, GitActionsPanel, PushForm, DiffView and JsonTree, so Rollup places them in the entry chunk: the built index-*.js (about 280 KB) contains the strings "Add secret", "Replace value", "Force push" and `orchestrator_only`, while SecretsPage-*.js is 2.9 KB. This contradicts SPEC.md, "Code splitting" ("entry bundle carries only what a first paint needs") and the comment at src/App.tsx:9-16, which already explains why App does not import the pages barrel. utils/index.ts has the same hazard (it re-exports diff.ts and the router-dependent useReturnTo).
The barrels guarantee nothing in return: components/index.ts serves 25 imports against 77 deep imports and omits Markdown (which exports `MarkdownBody` from Markdown.tsx), admin/*, DiffBody and ChangesFileList; tasks/index.ts re-exports about 70 names of which 5 are imported through it, with ./taskStore re-exported in three statements and ./taskChrome in two; pages/index.ts is imported nowhere; utils/index.ts is incomplete (formatTokens, shortId and all of sharedDir.ts are imported by path).
Dead code confirmed by word-boundary grep over src/ and tests/:
- pages/PlaceholderPage.tsx and pages/index.ts; PROJECTS_REFETCH_MS, CONFIRMATION_MS, DEFAULT_PROJECT_TAB and the *Props re-exports in pages/project/index.ts; the redundant null guard at ProjectsPage.tsx:247.
- services: getHealth and Health (a 503 body would be discarded by toApiError anyway, and the type sits in services/ unlike every other shape), getProfile, the never-passed `init?: RequestInit` on the api* functions (see su83c before deleting: it is where AbortSignal would go), TaskFilters and the filter half of listTasks, ListEventsParams/ListSessionsParams/ListSecretsParams.
- tasks: selectTaskByNumber, buildTaskLink (CopyLinkButton builds origin + path itself), TaskSessionLink, ReviewLabel.status (tests only). session: `reset` and disposeSessionStore have no production caller (addgw gives them one), lineCount (json.ts:31), the never-read `depth` prop of MessageRow.
- types barrel: GitConflictError, ReplaceSecretRequest, ClientMessageType, ServerMessageType, AgentEventKind, AgentEventBase, GitOp, McpServerStatus, BranchKind, TaskActor, DiffFile; utils barrel: BACKOFF_JITTER, PASSWORD_MIN_LENGTH, PASSWORD_MAX_LENGTH, PatchHunk. (Used inside their own files; only the barrel export is dead.)
- Name collisions: two exported `TaskRef` types (services/tasks.ts:17 and utils/taskRef.ts:9); the `SessionActions` store interface vs the SessionActions component (session/index.ts already works around it); the `Comment` type shadows the DOM global, so a forgotten import type-checks against the DOM class.

Acceptance: nothing on the entry path (main, App, AuthBootstrap, eager pages, PageLayout, route guards) imports a barrel that reaches feature UI; decide per barrel whether to enforce it (lint rule) or delete it, and say which in ARCHITECTURE.md, "Frontend architecture". A build check asserts the entry chunk does not contain the secrets/git UI (a string grep on dist in CI, or a size budget) so it cannot regress. The dead code above is deleted (re-grep first: main has moved), collisions renamed. npm run build output before and after recorded in the commit message.

References: frontend/src/AuthBootstrap.tsx, App.tsx, components/index.ts, utils/index.ts, tasks/index.ts, pages/index.ts, types/index.ts; frontend/vite.config.ts. Contract: SPEC.md, "Frontend", "Code splitting"; README.md, "Development" if a check is added to CI.