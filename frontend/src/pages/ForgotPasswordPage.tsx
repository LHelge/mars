// `/forgot-password` (`SPEC.md`, "Auth (`/api/auth`)": `POST
// /auth/request-password-reset` always answers 204).
//
// The orchestrator never reveals whether an identifier exists, and throttled
// requests answer 204 as well, so the page shows one confirmation for every
// outcome. Only a request that never reached the orchestrator is reported.

import { useState, type FormEvent } from "react";
import { Link } from "react-router";
import { Alert, AuthLayout, FormField, SubmitButton } from "../components";
import { useFormSubmit } from "../hooks";
import { ApiError, requestPasswordReset } from "../services";

export function ForgotPasswordPage() {
  const [identifier, setIdentifier] = useState("");
  const [identifierError, setIdentifierError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);

  const { submit, loading, error } = useFormSubmit(async () => {
    try {
      await requestPasswordReset(identifier.trim());
    } catch (caught) {
      if (!(caught instanceof ApiError)) {
        // Never reached the orchestrator, or a bug here: both are worth
        // saying out loud, and `errorMessage` names them.
        throw caught;
      }
      // A status body would only tell the visitor something the endpoint is
      // designed not to tell them. Fall through to the confirmation.
    }
    setSent(true);
  });

  function onSubmit(event: FormEvent) {
    event.preventDefault();

    if (identifier.trim() === "") {
      setIdentifierError("Enter your username or email");
      return;
    }
    setIdentifierError(null);
    void submit();
  }

  if (sent) {
    return (
      <AuthLayout title="Check your email">
        <div className="flex flex-col gap-4">
          <Alert kind="success">
            If that account exists, a reset link has been sent.
          </Alert>
          <Link to="/login" className="text-console-accent text-sm hover:underline">
            Back to sign in
          </Link>
        </div>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout
      title="Forgot your password?"
      footer={
        <Link to="/login" className="text-console-accent hover:underline">
          Back to sign in
        </Link>
      }
    >
      <form onSubmit={onSubmit} className="flex flex-col gap-4" noValidate>
        {error !== null && <Alert kind="error">{error}</Alert>}

        <FormField
          label="Username or email"
          name="identifier"
          value={identifier}
          onChange={(value) => {
            setIdentifier(value);
            setIdentifierError(null);
          }}
          error={identifierError ?? undefined}
          hint="We send a reset link to the address on the account."
          autoComplete="username"
          autoFocus
          disabled={loading}
        />

        <SubmitButton loading={loading}>Send reset link</SubmitButton>
      </form>
    </AuthLayout>
  );
}
