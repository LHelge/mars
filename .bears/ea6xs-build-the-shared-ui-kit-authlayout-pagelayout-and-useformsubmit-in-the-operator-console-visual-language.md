---
id: ea6xs
title: Build the shared UI kit, AuthLayout, PageLayout and useFormSubmit() in the operator-console visual language
status: in_progress
priority: P1
created: "2026-09-16T20:40:28.452915170Z"
updated: "2026-09-19T10:47:58.894608632Z"
tags:
  - frontend
depends_on:
  - t85sb
parent: "2f5u2"
attempts: 1
---

## Summary
Establish the visual language and the reusable pieces every page in this and the later frontend epics composes: `FormField`, `SubmitButton`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader`, the two layouts `AuthLayout` (centred card for unauthenticated pages) and `PageLayout` (top navigation with Dashboard, Projects, Secrets, Settings, Admin for admins, and the signed-in user with a logout action), and the `useFormSubmit()` hook that owns form loading and error state. This task invokes the `/frontend-design` skill first and writes the resulting tokens into `src/index.css` so later work extends rather than restyles.

## Documents
- `SPEC.md` "Frontend", Structure: `components/` reusable UI: FormField, SubmitButton, Alert, LoadingState, EmptyState, PageLayout, AuthLayout, ProtectedRoute, AdminRoute; `hooks/` useAuth, useFormSubmit.
- `SPEC.md` "Frontend", Routes: `/` (DashboardPage), `/projects`, `/secrets`, `/settings`, `/admin` — these are the navigation entries.
- `CLAUDE.md` "Frontend conventions": functional components with hooks only, named exports; `useFormSubmit()` for form loading and error state; shared layouts `AuthLayout`, `PageLayout`; shared UI `FormField`, `SubmitButton`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader`; Heroicons (`@heroicons/react/24/outline`); "Invoke the `/frontend-design` skill before creating or reshaping UI. The tone is a focused, dense operator console: dark-friendly, monospace where content is code or logs, quiet colour reserved for state (running, parked, failed, needs human)."
- `SPEC.md` "Non-goals": "Mobile layouts beyond 'does not break'."

## Acceptance criteria
- [ ] `frontend/src/index.css` defines the design tokens as CSS variables under `@theme` (Tailwind 4): background/surface/border/text scales for a dark-first console, one accent, and the four state colours named `--color-state-running`, `--color-state-parked`, `--color-state-failed`, `--color-state-human`, plus `--font-mono`. `prefers-color-scheme: light` produces a usable light variant; no other colour is used for decoration.
- [ ] `frontend/src/components/FormField.tsx`: `{ label, name, type?, value, onChange, error?, hint?, autoComplete?, required?, autoFocus?, disabled?, children? }`; renders `<label for>` + input (or `children` for a custom control such as a select/textarea), the error under the field with `aria-describedby`, and `aria-invalid` when `error` is set.
- [ ] `SubmitButton.tsx`: `{ loading, children, disabled?, variant?: "primary" | "danger" | "ghost" }`; disabled while loading, shows a spinner and keeps its width.
- [ ] `Alert.tsx`: `{ kind: "error" | "success" | "info" | "warning", children, onDismiss? }` with `role="alert"` for `error`, `role="status"` otherwise.
- [ ] `LoadingState.tsx` (`{ label? }`, `role="status"`), `EmptyState.tsx` (`{ title, description?, action? }`), `SectionHeader.tsx` (`{ title, description?, actions? }`).
- [ ] `AuthLayout.tsx`: `{ title, children, footer? }` — centred, narrow card with the product name `Mars`; used by login, invite, forgot/reset and change-password pages.
- [ ] `PageLayout.tsx`: `{ title?, actions?, children }` — top bar with nav links (`NavLink` from `react-router`, active style) to `/` (Dashboard), `/projects`, `/secrets`, `/settings` and `/admin` (rendered only when `useAuth().isAdmin`), the current `username` in monospace and a `Log out` button calling `useAuth().logout()`. Content area is full-width with a dense max-width; no sidebar.
- [ ] `frontend/src/hooks/useFormSubmit.ts`: `useFormSubmit<TArgs extends unknown[]>(action: (...args: TArgs) => Promise<void>) => { submit: (...args: TArgs) => Promise<void>; loading: boolean; error: string | null; clearError(): void }`; a thrown `ApiError` sets `error` to its `error` text, any other error sets `Something went wrong` and `console.error`s it; `submit` ignores re-entrant calls while loading.
- [ ] `StatusBadge.tsx` (`{ state: "running" | "parked" | "failed" | "human" | "done" | "creating" | "cloning" | "ready" | "error", label? }`) is added here because the dashboard, admin and later session lists all need one; only the four state tokens carry colour, the rest are neutral.
- [ ] Every component is a named export, re-exported from `components/index.ts` and `hooks/index.ts`; `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/index.css`, `frontend/src/components/{FormField,SubmitButton,Alert,LoadingState,EmptyState,SectionHeader,StatusBadge,AuthLayout,PageLayout,index}.tsx`, `frontend/src/hooks/{useFormSubmit,index}.ts`.
- Run the `/frontend-design` skill before writing any JSX (CLAUDE.md). If the skill is not installed in the working environment, apply the tone sentence in CLAUDE.md literally and record the tokens chosen in a short comment at the top of `index.css`.
- Tailwind utility classes inline; no CSS modules, no component library. Icons only from `@heroicons/react/24/outline`.
- `PageLayout` reads `useAuth()`; it must render correctly when `user` is still `null` (startup before `/users/me` resolves) by showing a placeholder instead of crashing.
- `useFormSubmit` is generic over the action's arguments so pages can call `submit(event)` or `submit(values)`.

## Edge cases
- `FormField` with `type="password"` sets `autoComplete` to `current-password` or `new-password` as the page passes it; never default to `on`.
- `Alert` for `error` must be announced by screen readers (`role="alert"`); success uses `role="status"`.
- `PageLayout` nav collapses to a horizontal scroll below `md`; nothing must break at 360 px width.
- Do not build `ProtectedRoute`/`AdminRoute` here (next task) and do not build any page.

## Testing
- Vitest + `@testing-library/react` (add as dev dependency here): `useFormSubmit` sets `loading` during the action, surfaces `ApiError.error` text, ignores a second concurrent submit, and `clearError` resets; `PageLayout` hides the Admin link for a non-admin user and shows it for an admin (stub `services/auth` state with `installSession`).
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements `CLAUDE.md` "Frontend conventions" and `SPEC.md` "Frontend" structure as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `src/index.css` base tokens from task ncv5g are extended, not replaced.