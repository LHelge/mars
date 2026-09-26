// The client-side half of the password rule in `SPEC.md`, "User-facing
// features": "passwords are 10–128 characters". Shared by the reset, invite
// and change-password pages so all three say the same thing.
//
// Length is counted in UTF-16 code units, which is what `String.length` gives;
// the orchestrator's rule is the authoritative one, so a server rejection is
// still surfaced rather than assumed impossible.

export const PASSWORD_MIN_LENGTH = 10;
export const PASSWORD_MAX_LENGTH = 128;

/** The one message every password field shows when the length is wrong. */
export const PASSWORD_LENGTH_MESSAGE = "Password must be 10–128 characters";

/** Returns the field error for `value`, or `null` when it is acceptable. */
export function validatePassword(value: string): string | null {
  if (
    value.length < PASSWORD_MIN_LENGTH ||
    value.length > PASSWORD_MAX_LENGTH
  ) {
    return PASSWORD_LENGTH_MESSAGE;
  }
  return null;
}
