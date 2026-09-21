---
id: "579dz"
title: Frontend technical review
type: epic
status: open
priority: P1
created: "2026-09-21T10:42:56.968006926Z"
updated: "2026-09-21T10:57:02.857686494Z"
tags:
  - frontend
  - technical-review
---

Address the findings of two frontend technical reviews requested by the user, merged on 2026-09-21: the first produced eleven tasks (six correctness findings, five maintainability/accessibility improvements); the second, a six-way read of all ~24,600 non-test lines of frontend/src, confirmed most of those independently, extended them, and added twenty-one tasks of its own.

Scope: task draft ownership, authentication refresh coordination (in-tab and cross-tab), live-stream lifetime across token refresh, resource identity, session-store lifetime, streamed-message folding, history retry, socket failure states, the unified-diff parser, background-refetch failure handling and query retry policy, task-stream replay and invalidation cost, secrets error text and plaintext lifetime, markdown image loading and CSP, shared fields, form submission and error-message conventions, launch-form duplication, per-row mutations, entry-bundle contents, dead code, type strictness, lifecycle comments, stream boundary validation, and task-drawer keyboard focus.

Children by priority.
P1 bugs: wsckz (edit baseline), c9tju (key the drawer by task), 4srw8 (single-flight refresh) -> xreap (no stream teardown on ordinary refresh), auzv5 (diff parser), hqbzq (failed refetch blanks the page; 4xx retry), 3rgt7 (streamed answer splits).
P2 bugs: tmd2v (pin hand-off; MoveToState), 4x7jq (secrets 409 text, lingering value), 9m8dn (markdown images, CSP), unh3n (task stream replay and invalidation), cpv5f (side panel, terminal), 7f5m4 (transcript and composer state), 4tb2c (per-row mutations, current user), wast2 (error-message helper), h53mn (launch forms), su83c (cross-tab auth, logout, return-to), addgw (session stores on sign-out), 5453d (history retry), ntepg (socket rejection state), 3q5pg (boundary validation), h2uej (memoised transcript rows), qh8ue (entry chunk, dead code).
P2 refactor/accessibility: s9rxc (field primitives, button props), 9c5rg (submission convention), kzvp9 (drawer focus, Escape).
P3: td3dh (project page fixes), gm6te (component fixes), cdeu8 (duplicated form bodies), 6wv7e (shared UI primitives), wpju2 (type strictness, URL building), uz3wt (comments, last).

Review evidence: first review — ESLint, TypeScript compilation and 506 unit tests passed; a temporary component regression test confirmed that TaskEditForm submits an old priority after a concurrent server update when the user edited only the title (removed). Second review — read-only; the three diff-parser failures were reproduced by running parseUnifiedPatch on hand-written patches, the entry-chunk contents were read from an existing dist/, and the coordinating session re-read the code path of every P1 finding and of the secrets, session-store and error-fallback findings; everything else was traced through source by one reviewer and needs focused regression coverage. Browser E2E tests were not run. main moved between the second review and the filing of its tasks (the agent-credentials frontend landed); findings in the changed files were re-checked, but line numbers in ProfileEditorPage.tsx, LaunchSessionForm.tsx, LaunchForTask.tsx and SecretsPage.tsx may have shifted.

What the reviews found sound and the work must preserve: SessionSocket and TaskStream as plain classes with injected factories and dispose discipline; the pure immutable fold shared by the live and history paths; the board's single-flight, generation-checked refresh ordering; Zustand selectors that return primitives or stable references; React-free, table-driven, unit-tested rule modules; defaults derived during render rather than synced by effects; useFormSubmit's in-flight guard; safeReturnTo; gcTime 0 + reset() for secret plaintext in mutations; auth gating that never flashes protected content.

Contracts: SPEC.md, "Authentication", "Frontend", "Tasks", "Secrets", "AgentEvent", "WebSocket: session stream", and "SSE: task stream"; ARCHITECTURE.md, "Frontend architecture" and "Git model"; CLAUDE.md, "Frontend conventions", "Testing expectations", and "Code quality".

Acceptance: complete the child tasks, preserve the existing services/query/store architecture, update affected documentation with behavior changes, and run the required frontend quality checks (auzv5 and 9m8dn also touch orchestrator/ and nginx/ and run those chains). Follow CLAUDE.md's epic implementation workflow when implementation is requested. This epic records authorized backlog work; implementation has not started.