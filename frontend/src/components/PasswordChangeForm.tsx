// The self-service password change (`SPEC.md`, "Users (`/api/users`)": `POST
// /users/{id}/password` with `{current_password?, password}` answers
// `{user, access_token}` for yourself), shared by `/change-password` and the
// settings page.
//
// `SPEC.md`, "Authentication": "A self-service change requires
// `current_password` and creates a replacement refresh token in that
// transaction, then returns its cookie and a matching access token after
// commit; this browser stays signed in." So the response is installed through
// `installSession` with the reason `password_change` — the one install that
// fires `onCredentialsReplaced` for a browser that stays put, letting the
// stream hooks reconnect with the fresh token. An ordinary refresh rotation
// installs as `refresh` and leaves open streams alone. The TanStack Query cache is *not*
// cleared: the same user is still authorised for everything it holds; only the
// current user is refreshed in place.
//
// Wrong or missing current password: the orchestrator
// (`orchestrator/src/routes/users.rs`) answers **400** `current password is
// incorrect` or `current password required`, never 401, so `apiClient`'s
// refresh-and-retry path is not involved and nothing here can be mistaken for
// token expiry. A new password equal to the current one is allowed by the
// spec and is not blocked here.

import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { useAuth, useFormSubmit } from "../hooks";
import { ApiError, changePassword, installSession, queryKeys } from "../services";
import { validatePassword } from "../utils";
import { Alert } from "./Alert";
import { FormField } from "./FormField";
import { SubmitButton } from "./SubmitButton";

/** The orchestrator's wording for a wrong current password. */
const CURRENT_PASSWORD_INCORRECT = "current password is incorrect";

export interface PasswordChangeFormProps {
  /** Run after the new pair is installed; the page navigates from here. */
  onSuccess?: () => void;
  /** False only where the server does not ask for it (an admin form). */
  requireCurrent?: boolean;
}

/**
 * Turns a failed change into the message the user should read. Everything the
 * form wants to say differently from the server is rewrapped as an `ApiError`,
 * which is the one shape `useFormSubmit` renders verbatim.
 */
function changeFailure(caught: unknown): unknown {
  if (caught instanceof ApiError) {
    if (caught.status === 400 && caught.error === CURRENT_PASSWORD_INCORRECT) {
      return new ApiError(caught.status, "Current password is incorrect");
    }
    // Any other 400 is the server's own rule — the authoritative one — and is
    // shown exactly as it was sent. A 401 never reaches here: `apiClient`
    // refreshes once and, if that fails, signs out.
    return caught;
  }
  if (caught instanceof TypeError) {
    // `fetch` rejects with a `TypeError` when it never reached the server.
    return new ApiError(0, "Orchestrator unreachable");
  }
  return caught;
}

export function PasswordChangeForm({
  onSuccess,
  requireCurrent = true,
}: PasswordChangeFormProps) {
  const { user } = useAuth();
  const queryClient = useQueryClient();

  const [currentPassword, setCurrentPassword] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [currentError, setCurrentError] = useState<string | null>(null);
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<string | null>(null);

  const { submit, loading, error } = useFormSubmit(async () => {
    if (user === null) {
      // Only reachable behind `ProtectedRoute`, which waits for `GET
      // /users/me`; nothing sensible to send without an id.
      throw new ApiError(0, "Not signed in");
    }

    let auth;
    try {
      auth = await changePassword(user.id, {
        ...(requireCurrent ? { current_password: currentPassword } : {}),
        password,
      });
    } catch (caught) {
      // Keep the current password so a mistyped new one costs one field, not
      // all three.
      setPassword("");
      setConfirm("");
      throw changeFailure(caught);
    }

    if (auth === undefined) {
      // 204 is the administrator's answer; changing your own password always
      // carries the replacement pair.
      throw new ApiError(0, "Password changed, but no new session was issued");
    }

    setCurrentPassword("");
    setPassword("");
    setConfirm("");

    installSession(auth, "password_change");
    // Same user, still authorised: only the current user is refreshed.
    queryClient.setQueryData(queryKeys.users.me(), auth.user);
    onSuccess?.();
  });

  function onSubmit(event: FormEvent) {
    event.preventDefault();

    const nextCurrentError =
      requireCurrent && currentPassword === ""
        ? "Enter your current password"
        : null;
    const nextPasswordError = validatePassword(password);
    const nextConfirmError =
      nextPasswordError === null && confirm !== password
        ? "Passwords do not match"
        : null;
    setCurrentError(nextCurrentError);
    setPasswordError(nextPasswordError);
    setConfirmError(nextConfirmError);
    if (
      nextCurrentError !== null ||
      nextPasswordError !== null ||
      nextConfirmError !== null
    ) {
      return;
    }

    void submit();
  }

  return (
    <form onSubmit={onSubmit} className="flex flex-col gap-4" noValidate>
      {error && <Alert kind="error">{error}</Alert>}

      {requireCurrent && (
        <FormField
          label="Current password"
          name="current_password"
          type="password"
          value={currentPassword}
          onChange={(value) => {
            setCurrentPassword(value);
            setCurrentError(null);
          }}
          error={currentError ?? undefined}
          autoComplete="current-password"
          autoFocus
          disabled={loading}
        />
      )}

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
        autoFocus={!requireCurrent}
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

      <SubmitButton loading={loading}>Change password</SubmitButton>
    </form>
  );
}
