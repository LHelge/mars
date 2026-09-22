// Choosing a password twice: the state and the checks behind
// `NewPasswordFields`, shared by the invite, the reset and the change-password
// forms (`SPEC.md`, "User-facing features": passwords are 10–128 characters).
//
// It sits apart from the component because a module that renders a component
// exports nothing else (`react-refresh/only-export-components`), and it stays
// small and imports nothing but `utils/password` because two of its three
// callers are routed eagerly and so are on the first-paint path
// (`ARCHITECTURE.md`, "Frontend architecture", Barrels and the first-paint
// path).

import { useState } from "react";

import { validatePassword } from "../utils/password";

/** What the two fields hold, and what they are refused with. */
export interface NewPasswordState {
  password: string;
  confirm: string;
  passwordError: string | null;
  confirmError: string | null;
  setPassword: (value: string) => void;
  setConfirm: (value: string) => void;
}

export interface NewPassword extends NewPasswordState {
  /** Shows what a submission would be refused for; `true` means send it. */
  validate: () => boolean;
  /** Empties both fields and their answers, after a refusal or a success. */
  reset: () => void;
}

export function useNewPassword(): NewPassword {
  const [password, setPasswordValue] = useState("");
  const [confirm, setConfirmValue] = useState("");
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<string | null>(null);

  return {
    password,
    confirm,
    passwordError,
    confirmError,
    // An answer describes the value it was raised against, so typing in the
    // field takes it away.
    setPassword: (value) => {
      setPasswordValue(value);
      setPasswordError(null);
    },
    setConfirm: (value) => {
      setConfirmValue(value);
      setConfirmError(null);
    },
    validate: () => {
      const nextPasswordError = validatePassword(password);
      // A password that is already refused says one thing, not two: "do not
      // match" under a password nobody could have used is noise.
      const nextConfirmError =
        nextPasswordError === null && confirm !== password
          ? "Passwords do not match"
          : null;
      setPasswordError(nextPasswordError);
      setConfirmError(nextConfirmError);
      return nextPasswordError === null && nextConfirmError === null;
    },
    reset: () => {
      setPasswordValue("");
      setConfirmValue("");
      setPasswordError(null);
      setConfirmError(null);
    },
  };
}
