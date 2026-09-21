// `/reset-password/:token` (`SPEC.md`, "Auth (`/api/auth`)": `POST
// /auth/reset-password` answers 204).
//
// `SPEC.md`, "Authentication": "Reset by link returns 204 without logging the
// user in; they then log in with the new password." So this page installs no
// session and ends on a link to `/login`. The token stays in the route param:
// it is never rendered, copied into a query string or logged.

import { useState, type FormEvent } from "react";
import { Link, useParams } from "react-router";
import { Alert, AuthLayout, FormField, SubmitButton } from "../components";
import { useFormSubmit } from "../hooks";
import { ApiError, resetPassword } from "../services";
import { validatePassword } from "../utils";

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

  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<string | null>(null);
  const [invalidLink, setInvalidLink] = useState(false);
  const [done, setDone] = useState(false);

  const { submit, loading, error } = useFormSubmit(async () => {
    try {
      await resetPassword(token ?? "", password);
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

  if (token === undefined || token === "" || invalidLink) {
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

    const nextPasswordError = validatePassword(password);
    const nextConfirmError =
      nextPasswordError === null && confirm !== password
        ? "Passwords do not match"
        : null;
    setPasswordError(nextPasswordError);
    setConfirmError(nextConfirmError);
    if (nextPasswordError !== null || nextConfirmError !== null) {
      return;
    }

    void submit();
  }

  return (
    <AuthLayout title="Choose a new password">
      <form onSubmit={onSubmit} className="flex flex-col gap-4" noValidate>
        {error !== null && <Alert kind="error">{error}</Alert>}

        <FormField
          label="New password"
          name="password"
          type="password"
          value={password}
          onChange={(value) => {
            setPassword(value);
            setPasswordError(null);
          }}
          error={passwordError ?? undefined}
          hint="10–128 characters."
          autoComplete="new-password"
          autoFocus
          disabled={loading}
        />

        <FormField
          label="Repeat new password"
          name="confirm"
          type="password"
          value={confirm}
          onChange={(value) => {
            setConfirm(value);
            setConfirmError(null);
          }}
          error={confirmError ?? undefined}
          autoComplete="new-password"
          disabled={loading}
        />

        <SubmitButton loading={loading}>Set password</SubmitButton>
      </form>
    </AuthLayout>
  );
}
