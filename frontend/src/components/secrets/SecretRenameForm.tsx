// The rename panel of one secret row (`PATCH /secrets/{id}`), split out of the
// row for the same reason as the replace form: the draft name belongs to the
// panel and starts again from the stored name every time the panel is opened.
//
// A refusal keeps the panel open so the name can be corrected — including the
// 409 that says the scope already holds an agent credential, which is shown in
// the server's own words (`SPEC.md`, "Secrets").

import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { logUnexpected } from "../../services/errorMessage";
import { patchSecret } from "../../services/secrets";
import type { SecretMeta } from "../../types";
import { validateSecretName } from "../../utils/secretName";
import { FieldShell } from "../FieldShell";
import { CONTROL } from "../fieldStyles";
import { SubmitButton } from "../SubmitButton";
import { secretErrorMessage } from "./messages";

export interface SecretRenameFormProps {
  secret: SecretMeta;
  onSaved: (updated: SecretMeta) => void;
  onError: (message: string) => void;
  onPendingChange: (pending: boolean) => void;
  onCancel: () => void;
}

export function SecretRenameForm({
  secret,
  onSaved,
  onError,
  onPendingChange,
  onCancel,
}: SecretRenameFormProps) {
  const [name, setName] = useState(secret.name);
  const [nameError, setNameError] = useState<string | null>(null);

  const rename = useMutation({
    mutationFn: (next: string) => patchSecret(secret.id, { name: next }),
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
    onSettled: () => {
      onPendingChange(false);
    },
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const invalid = validateSecretName(name);
    setNameError(invalid);
    if (invalid !== null) {
      return;
    }
    rename.mutate(name);
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label={`Rename ${secret.name}`}
      className="flex flex-col gap-2"
    >
      <FieldShell
        label={`New name for ${secret.name}`}
        name={`rename-${secret.id}`}
        error={nameError ?? undefined}
      >
        {(control) => (
          <input
            {...control}
            type="text"
            value={name}
            onChange={(event) => {
              setName(event.target.value.toUpperCase());
              setNameError(null);
            }}
            autoComplete="off"
            spellCheck={false}
            required
            className={`${CONTROL} max-w-sm`}
          />
        )}
      </FieldShell>
      <div className="flex gap-2">
        <SubmitButton loading={rename.isPending}>Save name</SubmitButton>
        <SubmitButton type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}
