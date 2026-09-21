// `/login` (`SPEC.md`, "Frontend", Routes; "Auth (`/api/auth`)": `POST
// /auth/login` answers `{user, access_token}`, or 429 while throttled).
//
// `login()` installs the session itself, so this page only decides where to go
// next: the forced password change for a seeded or reset account, otherwise
// the destination a guard stashed in router state (`SPEC.md`, "Frontend",
// Copy links) and `/` as the fallback.

import { useState, type FormEvent } from "react";
import { Link, Navigate, useNavigate } from "react-router";
import { Alert, AuthLayout, FormField, SubmitButton } from "../components";
import { useAuth, useFormSubmit } from "../hooks";
import { ApiError, errorMessage, login } from "../services";
import { useReturnTo } from "../utils";

const CHANGE_PASSWORD_PATH = "/change-password";

/**
 * The two refusals this page words itself. `SPEC.md`, "Auth (`/api/auth`)",
 * answers both with a status, so neither reading depends on the server's
 * prose; everything else is `errorMessage`'s one rule.
 */
function loginFailure(caught: unknown): string {
  if (caught instanceof ApiError) {
    if (caught.status === 401) {
      return "Invalid username or password";
    }
    if (caught.status === 429) {
      // The window is fixed at 15 minutes (`SPEC.md`, "User-facing features").
      return "Too many failed attempts. Try again in 15 minutes.";
    }
  }
  return errorMessage(caught);
}

export function LoginPage() {
  const navigate = useNavigate();
  const returnTo = useReturnTo();
  const { isAuthenticated } = useAuth();

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [usernameError, setUsernameError] = useState<string | null>(null);
  const [passwordError, setPasswordError] = useState<string | null>(null);

  const { submit, loading, error } = useFormSubmit(
    async () => {
      let auth;
      try {
        auth = await login({ username: username.trim(), password });
      } catch (caught) {
        // Keep the username so a mistyped password costs one field, not two.
        setPassword("");
        throw caught;
      }

      if (auth.user.must_change_password) {
        // Carry the destination across the forced change so the copied link
        // still opens afterwards.
        await navigate(CHANGE_PASSWORD_PATH, {
          state: returnTo === null ? undefined : { from: returnTo },
          replace: true,
        });
        return;
      }
      await navigate(returnTo ?? "/", { replace: true });
    },
    { mapError: loginFailure },
  );

  // A signed-in visitor has no business on this page; send them on without
  // flashing the form.
  if (isAuthenticated) {
    return <Navigate to={returnTo ?? "/"} replace />;
  }

  function onSubmit(event: FormEvent) {
    event.preventDefault();

    const nextUsernameError =
      username.trim() === "" ? "Enter your username" : null;
    const nextPasswordError = password === "" ? "Enter your password" : null;
    setUsernameError(nextUsernameError);
    setPasswordError(nextPasswordError);
    if (nextUsernameError !== null || nextPasswordError !== null) {
      return;
    }

    void submit();
  }

  return (
    <AuthLayout
      title="Sign in"
      footer={
        <div className="flex flex-col gap-1">
          <Link
            to="/forgot-password"
            className="text-console-accent hover:underline"
          >
            Forgot your password?
          </Link>
          <p>Accounts are created by invitation.</p>
        </div>
      }
    >
      <form onSubmit={onSubmit} className="flex flex-col gap-4" noValidate>
        {error !== null && <Alert kind="error">{error}</Alert>}

        <FormField
          label="Username"
          name="username"
          value={username}
          onChange={(value) => {
            setUsername(value);
            setUsernameError(null);
          }}
          error={usernameError ?? undefined}
          autoComplete="username"
          autoFocus
          disabled={loading}
        />

        <FormField
          label="Password"
          name="password"
          type="password"
          value={password}
          onChange={(value) => {
            setPassword(value);
            setPasswordError(null);
          }}
          error={passwordError ?? undefined}
          autoComplete="current-password"
          disabled={loading}
        />

        <SubmitButton loading={loading}>Sign in</SubmitButton>
      </form>
    </AuthLayout>
  );
}
