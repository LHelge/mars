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
// installs as `refresh` and leaves open streams alone. The TanStack Query cache
// is *not* cleared: the same user is still authorised for everything it holds,
// and the current user it answers with is installed in the one place the
// current user lives (`SPEC.md`, "Frontend", Rules).
//
// Wrong or missing current password: the orchestrator
// (`orchestrator/src/routes/users.rs`) answers **400** `current password is
// incorrect` or `current password required`, never 401, so `apiClient`'s
// refresh-and-retry path is not involved and nothing here can be mistaken for
// token expiry. A new password equal to the current one is allowed by the
// spec and is not blocked here.

import { useState, type FormEvent } from "react";
import { useAuth, useFormSubmit } from "../hooks";
import { ApiError } from "../services/apiClient";
import { installSession } from "../services/auth";
import { MessageError } from "../services/errorMessage";
import { changePassword } from "../services/users";
import { validatePassword } from "../utils/password";
import { Alert } from "./Alert";
import { FormField } from "./FormField";
import { SubmitButton } from "./SubmitButton";

/**
 * The orchestrator's wording for a wrong current password. The status alone
 * cannot tell it from the 400 of a new password that breaks a rule, so this
 * one reading is keyed on that prose; `SPEC.md`, "Frontend", Failure
 * messages, records the coupling. It is read in exactly one place, the catch
 * below, because it is the one refusal that belongs to a field rather than to
 * the form: any other 400 is the server's own rule — the authoritative one —
 * and is shown exactly as it was sent, and a 401 never reaches here, because
 * `apiClient` refreshes once and, if that fails, signs out.
 */
const CURRENT_PASSWORD_INCORRECT = "current password is incorrect";

function isWrongCurrentPassword(caught: unknown): boolean {
  return (
    caught instanceof ApiError &&
    caught.status === 400 &&
    caught.error === CURRENT_PASSWORD_INCORRECT
  );
}

export interface PasswordChangeFormProps {
  /** Run after the new pair is installed; the page navigates from here. */
  onSuccess?: () => void;
}

export function PasswordChangeForm({ onSuccess }: PasswordChangeFormProps) {
  const { user } = useAuth();

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
      throw new MessageError("Not signed in");
    }

    let auth;
    try {
      auth = await changePassword(user.id, {
        current_password: currentPassword,
        password,
      });
    } catch (caught) {
      if (isWrongCurrentPassword(caught)) {
        // The one field that was wrong is the one that is cleared, and the
        // refusal is shown on it: the new pair the user typed is fine and
        // retyping it would be the form's own fault. Handled, so it returns
        // rather than raising a form-level alert as well.
        setCurrentPassword("");
        setCurrentError("Current password is incorrect");
        return;
      }
      // Any other refusal is about the new password: keep the current one so
      // a mistyped new one costs one field, not all three.
      setPassword("");
      setConfirm("");
      throw caught;
    }

    if (auth === undefined) {
      // 204 is the administrator's answer; changing your own password always
      // carries the replacement pair.
      throw new MessageError("Password changed, but no new session was issued");
    }

    setCurrentPassword("");
    setPassword("");
    setConfirm("");

    // `installSession` is what makes the response the current user; there is
    // no second copy to keep in step (`SPEC.md`, "Frontend", Rules).
    installSession(auth, "password_change");
    onSuccess?.();
  });

  function onSubmit(event: FormEvent) {
    event.preventDefault();

    const nextCurrentError =
      currentPassword === "" ? "Enter your current password" : null;
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
      {error !== null && <Alert kind="error">{error}</Alert>}

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
