// A new password and its repetition, as the invite, reset and change-password
// forms all ask for it.
//
// The wording is the caller's, because the three pages mean different things
// by it — an invitee picks "Password", someone who already has one picks a
// "New password" — but the names, the autocomplete hints and the rule under
// them are the same everywhere, which is the point of having one of these.

import { FormField } from "./FormField";
import type { NewPasswordState } from "./newPassword";

export interface NewPasswordFieldsProps {
  fields: NewPasswordState;
  label: string;
  confirmLabel: string;
  disabled: boolean;
  /** Set where this pair is the first thing on the page to fill in. */
  autoFocus?: boolean;
}

export function NewPasswordFields({
  fields,
  label,
  confirmLabel,
  disabled,
  autoFocus = false,
}: NewPasswordFieldsProps) {
  return (
    <>
      <FormField
        label={label}
        name="password"
        type="password"
        value={fields.password}
        onChange={fields.setPassword}
        error={fields.passwordError ?? undefined}
        hint="10–128 characters."
        autoComplete="new-password"
        autoFocus={autoFocus}
        disabled={disabled}
      />

      <FormField
        label={confirmLabel}
        name="confirm"
        type="password"
        value={fields.confirm}
        onChange={fields.setConfirm}
        error={fields.confirmError ?? undefined}
        autoComplete="new-password"
        disabled={disabled}
      />
    </>
  );
}
