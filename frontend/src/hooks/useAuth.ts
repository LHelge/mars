// `SPEC.md`, "Frontend": "Auth state lives in `services/auth` with an in-memory
// access token mirrored to `localStorage`, and a `useAuth()` hook that
// subscribes to it."

import { useMemo, useSyncExternalStore } from "react";
import { getAuthState, logout, subscribe } from "../services/auth";
import type { User } from "../types";

export interface UseAuth {
  user: User | null;
  accessToken: string | null;
  isAuthenticated: boolean;
  isAdmin: boolean;
  mustChangePassword: boolean;
  logout: () => Promise<void>;
}

/**
 * Token role and flag snapshots are UI hints only; the backend always checks
 * the current user.
 */
export function useAuth(): UseAuth {
  // `getAuthState` returns the same object until something changes, so this is
  // a valid `useSyncExternalStore` snapshot on both client and server.
  const state = useSyncExternalStore(subscribe, getAuthState, getAuthState);

  return useMemo(
    () => ({
      user: state.user,
      accessToken: state.accessToken,
      isAuthenticated: state.accessToken !== null,
      isAdmin: state.user?.admin ?? false,
      mustChangePassword: state.user?.must_change_password ?? false,
      logout,
    }),
    [state],
  );
}
