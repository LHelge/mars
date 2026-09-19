// `SPEC.md`, "Frontend", Rules: "`ProtectedRoute` redirects a user whose
// current `must_change_password` is set to the change-password page. Token
// role/flag snapshots are UI hints only; the backend always checks the current
// user."
//
// The redirect is driven by the current user loaded from `GET /users/me`, never
// by decoding the JWT, and the blocked destination travels in router `state` so
// login and the password change can return to it.

import { Navigate, Outlet, useLocation } from "react-router";
import { useAuth } from "../hooks/useAuth";
import { safeReturnTo } from "../utils/returnTo";
import { LoadingState } from "./LoadingState";

const CHANGE_PASSWORD_PATH = "/change-password";

export function ProtectedRoute() {
  const location = useLocation();
  const { isAuthenticated, user } = useAuth();

  const from = safeReturnTo(location.pathname + location.search);
  const state = from === null ? undefined : { from };

  if (!isAuthenticated) {
    return <Navigate to="/login" state={state} replace />;
  }

  // Authenticated but `GET /users/me` has not answered yet: neither guard below
  // can be decided from a token snapshot, so wait.
  if (user === null) {
    return <LoadingState />;
  }

  if (user.must_change_password && location.pathname !== CHANGE_PASSWORD_PATH) {
    return <Navigate to={CHANGE_PASSWORD_PATH} state={state} replace />;
  }

  return <Outlet />;
}
