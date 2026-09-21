---
id: wast2
title: "One API error-to-message helper: network failures named, no empty error text, no forged ApiErrors"
status: done
priority: P2
created: "2026-09-21T10:54:08.553165925Z"
updated: "2026-09-21T19:25:48.179207872Z"
tags:
  - frontend
  - technical-review
  - bug
  - refactor
parent: "579dz"
attempts: 1
---

Problem:
- services/apiClient.ts toApiError falls back to `new ApiError(response.status, response.statusText)` when the body is not the JSON envelope. statusText is always "" under HTTP/2 and HTTP/3, and nginx listens on plain HTTP behind an operator-terminated TLS proxy, so h2 to the browser is the likely deployment. Orchestrator down, nginx answers 502 with HTML: the client builds ApiError(502, ""), useFormSubmit calls setError(""), `{error && <Alert>}` renders nothing — the button spins, stops, and the user sees no feedback (hooks/useFormSubmit.ts:48; render sites such as pages/LoginPage.tsx). The query-error sites that print error.message fail the same way.
- `caught instanceof ApiError ? caught.error : "Something went wrong"` is re-implemented about eleven times with five different policies: components/admin/errorMessage.ts, pages/project/messages.ts (shows 5xx text), pages/project/sharedDirMessages.ts (hides 5xx), components/secrets/messages.ts, tasks/TaskStatesEditor.tsx:580 (identical to projectErrorMessage), tasks/mergeRules.ts:67, tasks/handoffRules.ts:146, session/SessionActions.tsx:37, tasks/taskStore.ts:100, session/useSessionSocket.ts:74, hooks/useFormSubmit.ts:47-54, plus inline copies in SessionsTab.tsx and ProjectsPage.tsx. Only the admin copy maps TypeError to "Orchestrator unreachable"; elsewhere a network failure reads "Something went wrong", and DashboardPage.tsx, ProjectsPage.tsx, SettingsPage.tsx and tasks/HandoffDiff.tsx:67 print the browser's raw "Failed to fetch".
- useFormSubmit has no way to map an error, so pages forge server errors to get a string through: 13 `new ApiError(0|status, "...")` sites outside apiClient — the TypeError -> ApiError(0, "Orchestrator unreachable") wrapper five times (LoginPage.tsx:37-40, AcceptInvitePage.tsx:51-54, ResetPasswordPage.tsx:59-61, ForgotPasswordPage.tsx:23-26, components/PasswordChangeForm.tsx:56-59), LoginPage's loginFailure, InvitesPanel.tsx:97-107.
- Layering inversion: feature code in tasks/ imports projectErrorMessage and isNotFound from pages/project/messages (TaskDetail, TaskEditForm, RevisionForm, TaskActions, MoveToState); session/SessionPage imports isUuid from there while utils/taskRef.ts has its own copy of the regex.
- Formatters that console.error are called during render (TaskStatesEditor.tsx:580-586, pages/project/messages.ts:10-16), so a non-ApiError failure logs on every re-render while the alert is visible, i.e. on each keystroke.
- Behaviour keyed on server prose: ResetPasswordPage.tsx:17/:52 (`=== "invalid or expired token"`), sharedDirMessages.ts:40-46 (substring "name"/"path"); correct today, silently wrong after a reword.

Acceptance: one `errorMessage(caught, fallback?)` in services/ or utils/ that owns the rules — ApiError below 500 shows the server's words, 5xx policy decided once, TypeError/network reads "Orchestrator unreachable", an empty error never reaches the UI (status-based text such as "HTTP 502", or the unreachable wording for 502/503/504). Per-feature helpers only add their special cases on top of it. useFormSubmit accepts `{ mapError?: (e: unknown) => string }`, compares `error !== null`, and the forged ApiErrors go. isNotFound/isUuid move to a neutral module. Logging happens once where the failure is caught (mutation onError), not in render. Where behaviour keys on server text, prefer status or a stable code and note the coupling in SPEC.md if it must stay. Unit tests for the helper's table; a form test for the HTML-502 case.

References: files above. Contract: SPEC.md, error shape `{ status, error }` and "Frontend"; CLAUDE.md, "API conventions" and "Frontend conventions". 9c5rg builds its convention on this helper.