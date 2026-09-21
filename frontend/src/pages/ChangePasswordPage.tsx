// `/change-password` (`SPEC.md`, "Frontend", Routes), the forced first-login
// change of `SPEC.md`, "Authentication": "While the current user's
// `must_change_password` is true, every authenticated endpoint other than
// `POST /auth/login`, `POST /auth/logout`, `POST /auth/refresh`,
// `GET /users/me` and `POST /users/{id}/password` answers 403 with error
// `password change required`. The frontend routes such a user to the
// change-password page."
//
// The page therefore calls nothing but `POST /users/{id}/password`, and wears
// `AuthLayout` rather than `PageLayout`: the shell's navigation would fire
// gated requests that the 403 rule rejects while the flag is set.
//
// Afterwards it continues to the destination a guard stashed in router state
// (`SPEC.md`, "Frontend", Copy links: preserve the destination through login
// and any required first-login password change), so a copied link still opens.

import { useState } from "react";
import { useNavigate } from "react-router";
import { AuthLayout } from "../components/AuthLayout";
import { PasswordChangeForm } from "../components/PasswordChangeForm";
import { useAuth } from "../hooks";
import { useReturnTo } from "../utils/returnTo";

export function ChangePasswordPage() {
  const navigate = useNavigate();
  const returnTo = useReturnTo();
  const { user } = useAuth();

  // Read once on mount: a successful change clears the flag on the installed
  // user, and the page still has to say where it came from.
  const [forced] = useState(() => user?.must_change_password === true);

  function onSuccess() {
    // A user who came here of their own accord goes back to the settings page
    // that embeds the same form; a forced one lands on the dashboard.
    void navigate(returnTo ?? (forced ? "/" : "/settings"), { replace: true });
  }

  return (
    <AuthLayout title="Set a new password">
      <div className="flex flex-col gap-4">
        {forced && (
          <p className="text-console-muted text-xs">
            You must change your password before continuing.
          </p>
        )}
        <PasswordChangeForm onSuccess={onSuccess} />
      </div>
    </AuthLayout>
  );
}
