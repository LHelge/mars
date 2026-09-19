---
id: t85sb
title: Implement services/apiClient.ts, services/auth.ts with the token store and useAuth()
status: done
priority: P0
created: "2026-09-16T20:39:52.095665806Z"
updated: "2026-09-19T10:47:57.786602163Z"
tags:
  - frontend
  - auth
depends_on:
  - c3hjz
parent: "2f5u2"
attempts: 1
---

## Summary
Deliver the single HTTP path every component uses: `services/apiClient.ts` with `apiGet`/`apiPost`/`apiPut`/`apiPatch`/`apiDelete` that attach the bearer token, refresh exactly once on 401 and hand a failed refresh to the sign-out path; `services/auth.ts` holding the in-memory access token mirrored to `localStorage`, the current `User`, a subscription API and the seven `/api/auth` calls; and `hooks/useAuth.ts` built on `useSyncExternalStore`.

## Documents
- `SPEC.md` "Authentication": 15-minute JWT in the body, 30-day refresh token in an `HttpOnly` cookie (`SameSite=Lax`, `Path=/`); `POST /api/auth/refresh` rotates and returns `{user, access_token}`; a missing user or expired/revoked refresh token returns 401 and clears the cookie; "A 401 from refresh clears local authentication, closes other streams and routes to login instead of retrying indefinitely; transient network/server failures retain normal backoff."; "A successful self-service password change installs its new pair before reopening streams."
- `SPEC.md` "Auth (`/api/auth`)" table: `POST /auth/login` `{username, password}` → `{user, access_token}` (429 when throttled); `POST /auth/refresh` (cookie) → `{user, access_token}`; `POST /auth/logout` (cookie) → 204; `GET /auth/invite/{token}` → `{email, admin, expires_at}` (400 if expired, used or unknown); `POST /auth/accept-invite` `{token, username, password}` → `{user, access_token}` (201); `POST /auth/request-password-reset` `{identifier}` → 204 (always); `POST /auth/reset-password` `{token, password}` → 204.
- `SPEC.md` "REST API": errors are `{ status, error }`, git conflicts add `conflicts`; status table (400, 401, 403, 404, 409, 422, 429, 500).
- `SPEC.md` "Frontend", Rules: "Auth state lives in `services/auth` with an in-memory access token mirrored to `localStorage`, and a `useAuth()` hook that subscribes to it."; "Load `GET /users/me` at authenticated startup and refresh the current user after an authorization 403 so a demotion updates admin navigation."; "A failed refresh with 401 clears the access token from memory and `localStorage`, clears authenticated query and stream stores, closes streams and returns to login."
- `ARCHITECTURE.md` "User authentication and revocation".
- `CLAUDE.md` "Frontend conventions": all API calls through `src/services/`; components never call `fetch`; `useAuth()` for auth state.

## Acceptance criteria
- [ ] `frontend/src/services/apiClient.ts` exports `apiGet<T>(path, init?)`, `apiPost<T>(path, body?, init?)`, `apiPut<T>`, `apiPatch<T>`, `apiDelete(path)` and the class `ApiError extends Error { status: number; error: string; conflicts?: string[] }`. Paths are given without the `/api` prefix (`apiGet<User>("/users/me")`); the client prepends `/api`, sets `Content-Type: application/json` when a body is present, `credentials: "same-origin"` so the refresh cookie travels, and `Authorization: Bearer <token>` when a token is held.
- [ ] 204 and empty bodies resolve to `undefined`; non-2xx responses reject with `ApiError` built from the JSON body, or `{ status, error: response.statusText }` when the body is not JSON. Network failures reject with a `TypeError` untouched so callers can show "orchestrator unreachable".
- [ ] On a 401 from any request except `/auth/login`, `/auth/refresh`, `/auth/logout`, `/auth/accept-invite`, `/auth/request-password-reset`, `/auth/reset-password` and `/auth/invite/*`, the client calls `refreshAccessToken()` and retries the original request **once** with the new token. Concurrent 401s share one in-flight refresh promise (no refresh storm). A second 401 after the retry rejects with the `ApiError` and does not refresh again.
- [ ] `refreshAccessToken()` posts to `/auth/refresh`; on 401 it calls `signOut("refresh_failed")`, which clears the token and user from memory and `localStorage` and runs every handler registered with `onSignOut(handler)` (the router task registers navigation to `/login`; the session and board epics register their store resets and stream closes). A refresh that fails with a network error or 5xx does **not** sign out; it rejects and leaves the token in place ("transient failures retain normal backoff").
- [ ] On a 403 whose body is not `password change required`, the client fires the registered `onForbidden` handler (debounced, at most one call per 2 seconds) so the router task can reload `GET /users/me`; on a 403 whose body **is** `password change required` the client fires `onPasswordChangeRequired`. Both are no-ops until registered.
- [ ] `frontend/src/services/auth.ts` exports: `getAccessToken(): string | null`, `getCurrentUser(): User | null`, `installSession(auth: AuthResponse)` (sets token + user, writes `localStorage["mars.access_token"]`), `setCurrentUser(user: User)`, `clearAuth()`, `subscribe(listener): () => void`, `onSignOut(handler): () => void`, `signOut(reason: "user" | "refresh_failed")`, `isAuthenticated()`; and the endpoint functions `login(body)`, `logout()` (posts `/auth/logout`, ignores errors, then `signOut("user")`), `refreshAccessToken()`, `lookupInvite(token)`, `acceptInvite(body)`, `requestPasswordReset(identifier)`, `resetPassword(token, password)`.
- [ ] At module load the token is read back from `localStorage` so a page reload stays signed in; the user object is **not** persisted (the router task reloads `/users/me` at startup). The token is the only key written; nothing else goes to storage.
- [ ] `frontend/src/hooks/useAuth.ts` exports `useAuth(): { user: User | null; accessToken: string | null; isAuthenticated: boolean; isAdmin: boolean; mustChangePassword: boolean; logout(): Promise<void> }` using `useSyncExternalStore(subscribe, getSnapshot)` with a stable snapshot object (re-created only on change).
- [ ] `services/health.ts` from the scaffold now uses `apiGet("/health")`; it remains the only unauthenticated GET besides invite lookup.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/services/apiClient.ts`, `frontend/src/services/auth.ts`, `frontend/src/services/health.ts` (rewrite), `frontend/src/services/index.ts` (barrel), `frontend/src/hooks/useAuth.ts`, `frontend/src/hooks/index.ts`, `frontend/src/services/apiClient.test.ts`, `frontend/src/services/auth.test.ts`, `frontend/vite.config.ts`, `frontend/package.json`.
- Sketch of the retry loop:
  ```ts
  async function request<T>(method, path, body?, { retried = false } = {}): Promise<T> {
    const res = await fetch(`/api${path}`, { method, headers, body, credentials: "same-origin" });
    if (res.status === 401 && !retried && !isAuthPath(path)) {
      await sharedRefresh();            // one promise for concurrent callers
      return request(method, path, body, { retried: true });
    }
    ...
  }
  ```
- `sharedRefresh()` keeps a module-level `refreshing: Promise<void> | null`, clears it in `finally`.
- `auth.ts` state is a plain module-level object with a `Set<() => void>` of listeners; do not use Zustand here (the stores in `session/` and `tasks/` are Zustand; auth is deliberately framework-free so `apiClient` can import it without React).
- Keep the `ApiError` message equal to `error` so `useFormSubmit` can show `err.message` directly.
- Never log the token; never put it in a URL here (stream hooks in other epics add `?token=` themselves).

## Edge cases
- A request issued while a refresh is in flight and receiving 401 must await that same refresh, then retry once with the new token.
- `signOut` must be idempotent and safe to call from a handler (guard re-entrancy).
- `localStorage` may throw (private mode, quota): wrap reads and writes, degrade to memory-only.
- `logout()` must clear local state even when `POST /auth/logout` fails (network down).
- 429 from login is surfaced as `ApiError` with `status: 429` so `LoginPage` can show the throttle message.
- `apiDelete` may receive a JSON error body on 409 (last-admin rules); parse it.

## Testing
- Vitest with a stubbed `globalThis.fetch` (`vi.fn`): (1) bearer header attached when a token is held, absent otherwise; (2) 401 → one refresh → retry succeeds, `fetch` called three times in order; (3) 401 → refresh 401 → `signOut("refresh_failed")` runs registered handlers once, token gone from memory and `localStorage`; (4) three concurrent 401s trigger exactly one refresh call; (5) 401 on `/auth/login` is not retried; (6) refresh failing with a network error keeps the token and rejects; (7) 403 `password change required` fires `onPasswordChangeRequired`, other 403 fires `onForbidden` once within the debounce window; (8) `installSession` persists the token and `subscribe` listeners fire; (9) `useAuth` snapshot identity is stable across unrelated renders (via `@testing-library/react` `renderHook`, or a minimal manual test of `getSnapshot`).
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written (Vitest and its documentation are delivered by the frontend scaffold task).

## Assumes from other epics
- "Repository scaffolding, tooling and CI": Vite scaffold, `services/health.ts` placeholder, ESLint flat config and the Vitest runner with `npm run test:unit` (task ncv5g); the Frontend CI workflow already runs it (task xxufa).
- "Authentication, users, invites and email": the `/api/auth/*` endpoints with the statuses cited above.