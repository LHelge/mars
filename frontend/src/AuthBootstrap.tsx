// Authenticated startup and the three cross-cutting auth reactions of
// `SPEC.md`, "Frontend", Rules:
//
//   * "Load `GET /users/me` at authenticated startup" — a reload keeps the
//     token in `localStorage` but never the user, and every guard needs the
//     current `admin` and `must_change_password`, not a token snapshot.
//   * "refresh the current user after an authorization 403 so a demotion
//     updates admin navigation" — `onForbidden`.
//   * "A failed refresh with 401 clears the access token from memory and
//     `localStorage`, clears authenticated query and stream stores, closes
//     streams and returns to login" — `onSignOut`.
//
// SignOutRegistry: `services/auth` keeps the set of sign-out handlers and runs
// them in registration order. This component registers the two the application
// shell owns — clearing the TanStack Query cache and navigating to `/login`.
// The session and board epics register their own store resets and stream closes
// through `onSignOut` from their providers; nothing further is needed here for
// the "clears stores, closes streams" part of the rule. Navigation is the last
// thing this handler does, and React applies the resulting render only after
// the whole handler chain has run, so every reset lands before the login page
// mounts.

import { useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { Alert, LoadingState, SubmitButton } from "./components";
import {
  ApiError,
  getAccessToken,
  getCurrentUser,
  onForbidden,
  onPasswordChangeRequired,
  onSignOut,
  setCurrentUser,
  signOut,
} from "./services";
import { getMe } from "./services/users";
import { useTaskStore } from "./tasks/taskStore";

export interface AuthBootstrapProps {
  children: ReactNode;
}

type Phase = "loading" | "ready" | "unreachable";

function initialPhase(): Phase {
  // Nothing to load without a token, and nothing to reload when a login in this
  // same page load already installed the user.
  return getAccessToken() !== null && getCurrentUser() === null
    ? "loading"
    : "ready";
}

export function AuthBootstrap({ children }: AuthBootstrapProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [phase, setPhase] = useState<Phase>(initialPhase);
  const [attempt, setAttempt] = useState(0);

  const retry = useCallback(() => {
    setPhase("loading");
    setAttempt((value) => value + 1);
  }, []);

  useEffect(() => {
    const offForbidden = onForbidden(() => {
      // A demotion or a scope the user just lost: re-read the user so the nav
      // and `AdminRoute` follow. A failure here is not worth surfacing.
      void getMe()
        .then(setCurrentUser)
        .catch(() => undefined);
    });
    const offPasswordChange = onPasswordChangeRequired(() => {
      void navigate("/change-password");
    });
    const offSignOut = onSignOut(() => {
      queryClient.clear();
      // The board snapshot is one user's view of a project; in-flight reads
      // are discarded with it.
      useTaskStore.getState().reset();
      void navigate("/login", { replace: true });
    });

    return () => {
      offForbidden();
      offPasswordChange();
      offSignOut();
    };
  }, [navigate, queryClient]);

  useEffect(() => {
    // Nothing to load, and `initialPhase`/`retry` have already put the phase
    // where it belongs — an effect body never sets state synchronously.
    if (getAccessToken() === null || getCurrentUser() !== null) {
      return;
    }

    let cancelled = false;

    void getMe()
      .then((user) => {
        if (cancelled) {
          return;
        }
        setCurrentUser(user);
        setPhase("ready");
      })
      .catch((error: unknown) => {
        if (cancelled) {
          return;
        }
        // 401 here is already past `apiClient`'s single refresh attempt, so the
        // session is gone for good — a refresh that answered 401 has signed out
        // on its own, hence the guard against a second round.
        if (error instanceof ApiError && error.status === 401) {
          if (getAccessToken() !== null) {
            signOut("refresh_failed");
          }
          setPhase("ready");
          return;
        }
        // A `TypeError` from `fetch` or a 5xx: the orchestrator is unreachable
        // or unwell. Only a 401 signs out, so keep the token and offer a retry;
        // rendering the routes without a user would leave `ProtectedRoute`
        // waiting forever.
        setPhase("unreachable");
      });

    return () => {
      cancelled = true;
    };
  }, [attempt]);

  if (phase === "loading") {
    return <LoadingState label="Signing in" />;
  }

  if (phase === "unreachable") {
    return (
      <main className="mx-auto flex max-w-sm flex-col gap-3 p-8">
        <Alert kind="error">orchestrator unreachable</Alert>
        <div>
          <SubmitButton type="button" onClick={retry}>
            Retry
          </SubmitButton>
        </div>
      </main>
    );
  }

  return <>{children}</>;
}
