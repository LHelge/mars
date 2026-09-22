// `/reset-password/:token` (`SPEC.md`, "Auth (`/api/auth`)": `POST
// /auth/reset-password` answers 204).
//
// `SPEC.md`, "Authentication": "Reset by link returns 204 without logging the
// user in; they then log in with the new password." So this page installs no
// session and ends on a link to `/login`. The token stays in the route param:
// it is never rendered, copied into a query string or logged.
//
// The route's param is a `string | undefined` and the form needs a token, so
// the guard is a component boundary: `ResetPasswordPage` decides whether there
// is a link to act on, and `ResetPasswordForm` is only ever mounted with one.

import { useState, type FormEvent } from "react";
import { Link, useParams } from "react-router";
import { Alert } from "../components/Alert";
import { AuthLayout } from "../components/AuthLayout";
import { NewPasswordFields } from "../components/NewPasswordFields";
import { useNewPassword } from "../components/newPassword";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks";
import { ApiError } from "../services/apiClient";
import { resetPassword } from "../services/auth";

/**
 * The 400 body every reset-token rejection shares (unknown, used or expired).
 * The status alone cannot tell it from the 400 of a password that breaks the
 * server's own rule, so this one reading is keyed on the orchestrator's
 * wording; `SPEC.md`, "Frontend", Failure messages, records the coupling.
 */
const INVALID_RESET_TOKEN = "invalid or expired token";

function InvalidLink() {
  return (
    <AuthLayout title="Choose a new password">
      <div className="flex flex-col gap-4">
        <Alert kind="error">This reset link is invalid or has expired.</Alert>
        <Link
          to="/forgot-password"
          className="text-console-accent text-sm hover:underline"
        >
          Request a new link
        </Link>
      </div>
    </AuthLayout>
  );
}

export function ResetPasswordPage() {
  const { token } = useParams<{ token: string }>();

  if (token === undefined || token === "") {
    return <InvalidLink />;
  }

  return <ResetPasswordForm token={token} />;
}

function ResetPasswordForm({ token }: { token: string }) {
  const chosen = useNewPassword();
  const [invalidLink, setInvalidLink] = useState(false);
  const [done, setDone] = useState(false);

  const { submit, loading, error } = useFormSubmit(async () => {
    try {
      await resetPassword(token, chosen.password);
    } catch (caught) {
      if (
        caught instanceof ApiError &&
        caught.status === 400 &&
        caught.error === INVALID_RESET_TOKEN
      ) {
        setInvalidLink(true);
        return;
      }
      // Any other 400 is the server's own password rule, which is the
      // authoritative one: it is shown verbatim and the link is still good.
      // Everything else — a network failure included — is `errorMessage`'s.
      throw caught;
    }
    setDone(true);
  });

  if (invalidLink) {
    return <InvalidLink />;
  }

  if (done) {
    return (
      <AuthLayout title="Choose a new password">
        <div className="flex flex-col gap-4">
          <Alert kind="success">
            Password updated. Sign in with your new password.
          </Alert>
          <Link
            to="/login"
            className="text-console-accent text-sm hover:underline"
          >
            Back to sign in
          </Link>
        </div>
      </AuthLayout>
    );
  }

  function onSubmit(event: FormEvent) {
    event.preventDefault();
    if (!chosen.validate()) {
      return;
    }
    void submit();
  }

  return (
    <AuthLayout title="Choose a new password">
      <form onSubmit={onSubmit} className="flex flex-col gap-4" noValidate>
        {error !== null && <Alert kind="error">{error}</Alert>}

        <NewPasswordFields
          fields={chosen}
          label="New password"
          confirmLabel="Repeat new password"
          disabled={loading}
          autoFocus
        />

        <SubmitButton loading={loading}>Set password</SubmitButton>
      </form>
    </AuthLayout>
  );
}
