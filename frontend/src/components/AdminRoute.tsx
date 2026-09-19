// The administrator guard, always nested inside `ProtectedRoute`, so the user
// is loaded by the time it renders.
//
// A demotion arrives as an authorization 403, which refreshes the current user
// (`SPEC.md`, "Frontend", Rules). The just-demoted administrator then sees why
// the page went away instead of being bounced somewhere else, and the backend
// checks authorisation regardless — the flag here is a UI hint.

import { Outlet } from "react-router";
import { useAuth } from "../hooks/useAuth";
import { Alert } from "./Alert";
import { LoadingState } from "./LoadingState";
import { PageLayout } from "./PageLayout";

export function AdminRoute() {
  const { user } = useAuth();

  if (user === null) {
    return <LoadingState />;
  }

  if (!user.admin) {
    return (
      <PageLayout title="Forbidden">
        <Alert kind="error">Administrator access required</Alert>
      </PageLayout>
    );
  }

  return <Outlet />;
}
