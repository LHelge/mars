---
id: su83c
title: Propagate sign-out and token changes across tabs; make logout and re-login after expiry behave
status: done
priority: P2
created: "2026-09-21T10:54:35.034790374Z"
updated: "2026-09-21T20:46:10.204191027Z"
tags:
  - frontend
  - technical-review
  - bug
  - auth
depends_on:
  - "4srw8"
  - xreap
parent: "579dz"
attempts: 1
---

Problem:
- services/auth.ts reads the token from localStorage once at module load (:65) and has no `storage` listener. Log out in tab A: tab B keeps its in-memory JWT; logout revokes only the refresh token and leaves auth_version unchanged, so B's REST works until expiry (up to 15 min) and B's already-open WebSocket — terminal included — keeps working indefinitely, since streams re-check auth_version, not the refresh token (SPEC.md, "Authentication").
- The same absence is the cross-tab half of the refresh race in 4srw8: two visible tabs that loaded the same token expire at the same instant and both present the same rotating cookie; the loser signs out.
- logout() awaits apiPost("/auth/logout") before signOut("user") (auth.ts:191-199) and no fetch in apiClient has a timeout, so with a blackholed orchestrator the "Log out" button does nothing for minutes. No service function accepts an AbortSignal at all (the `init` parameter on the api* functions is never passed), so TanStack's cancellation never reaches the network: a changed head cannot cancel an in-flight getDiff (up to 1 MiB, syncs the session head server-side first) or listEvents.
- AuthBootstrap's sign-out handler ignores `reason` and always navigates to /login with replace and no state.from (AuthBootstrap.tsx:77-83), pre-empting ProtectedRoute's own <Navigate state={{from}}>. After a refresh_failed sign-out the user logs in again and lands on / rather than the task or session they were on (SPEC.md, "Copy links" describes the preservation for a normal login).

Acceptance: a `storage` listener on the token key signs the tab out when the value is removed and adopts a different value (closing/reopening streams as a credential replacement per the rules settled in 4srw8 and the stream-teardown task). The refresh is serialised across tabs (navigator.locks.request around it, re-reading localStorage inside the lock and adopting a newer token instead of refreshing) or an equivalent; a short server-side reuse grace is the alternative and needs a SPEC.md change and an ADR. Sign-out is local first, the cookie-revoking request fire-and-forget or bounded by AbortSignal.timeout. TanStack's `signal` is threaded through at least getDiff and listEvents. A refresh_failed sign-out carries `from: safeReturnTo(pathname + search)`. Tests with two simulated tabs (storage events) for logout and for refresh; logout with a hanging request; return-to after expiry.

References: frontend/src/services/auth.ts, apiClient.ts, git.ts, sessions.ts; frontend/src/AuthBootstrap.tsx; frontend/src/utils/returnTo.ts; frontend/src/components/ProtectedRoute.tsx. Contract: SPEC.md, "Authentication", "Copy links" and "Frontend", Rules.