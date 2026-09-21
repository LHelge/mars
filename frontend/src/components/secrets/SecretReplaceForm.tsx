// The replace-value panel of one secret row (`PUT /secrets/{id}`).
//
// It is a component of its own so that the plaintext lives in a state that
// cannot outlive the panel: clicking `Replace value` again, switching to
// `Rename` or `Uses`, cancelling, or a save that succeeded all unmount this
// form, and the typed value goes with it (`CLAUDE.md`, rule 3). The mutation
// is here for the same reason — `gcTime: 0` plus `reset()` drops the copy the
// mutation cache keeps in `variables`, and an unmounted mutation observer has
// nothing left to hold.

import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { logUnexpected } from "../../services/errorMessage";
import { replaceSecretValue } from "../../services/secrets";
import type { SecretMeta } from "../../types";
import { FieldShell } from "../FieldShell";
import { CONTROL } from "../fieldStyles";
import { SubmitButton } from "../SubmitButton";
import { secretErrorMessage } from "./messages";

export interface SecretReplaceFormProps {
  secret: SecretMeta;
  /** The metadata the server answered with; the row applies it. */
  onSaved: (updated: SecretMeta) => void;
  /** A refusal, in the words the row shows above the panel. */
  onError: (message: string) => void;
  /** Keeps the row's own actions disabled while the request is in flight. */
  onPendingChange: (pending: boolean) => void;
  onCancel: () => void;
}

export function SecretReplaceForm({
  secret,
  onSaved,
  onError,
  onPendingChange,
  onCancel,
}: SecretReplaceFormProps) {
  const [value, setValue] = useState("");

  const replace = useMutation({
    mutationFn: (next: string) => replaceSecretValue(secret.id, next),
    onMutate: () => {
      onPendingChange(true);
    },
    onSuccess: (updated) => {
      onSaved(updated);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      onError(secretErrorMessage(caught));
    },
    // The mutation cache keeps `variables` — here the plaintext — for as long
    // as the mutation lives, so it is dropped the moment the request settles.
    gcTime: 0,
    onSettled: () => {
      setValue("");
      onPendingChange(false);
      replace.reset();
    },
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    replace.mutate(value);
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label={`Replace the value of ${secret.name}`}
      className="flex flex-col gap-2"
    >
      <FieldShell
        label={`New value for ${secret.name}`}
        name={`replace-${secret.id}`}
      >
        {(control) => (
          <textarea
            {...control}
            rows={3}
            value={value}
            onChange={(event) => {
              setValue(event.target.value);
            }}
            autoComplete="off"
            spellCheck={false}
            required
            className={CONTROL}
          />
        )}
      </FieldShell>
      <div className="flex gap-2">
        <SubmitButton loading={replace.isPending}>Save value</SubmitButton>
        <SubmitButton type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}
