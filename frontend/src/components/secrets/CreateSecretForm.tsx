// `POST /secrets` (`SPEC.md`, "Secrets"): the one place in the UI where a
// secret value is typed. The value lives in component state for exactly as
// long as the request is in flight — `onSettled` clears it whether the request
// succeeded or failed, and a success unmounts the textarea with the form reset
// (`CLAUDE.md`, rule 3). It is never put in a query cache, a URL or
// `localStorage`.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { invalidateSecretQueries } from "../../secrets/invalidate";
import { createSecret } from "../../services/secrets";
import { queryKeys } from "../../services/queryKeys";
import type { CreateSecretRequest, SecretMeta, SecretScope } from "../../types";
import { validateSecretName } from "../../utils/secretName";
import { Alert } from "../Alert";
import { FieldShell } from "../FieldShell";
import { CONTROL } from "../fieldStyles";
import { FormField } from "../FormField";
import { SubmitButton } from "../SubmitButton";
import {
  ORCHESTRATOR_ONLY_HINT,
  SECRET_NAME_HINT,
  secretErrorMessage,
} from "./messages";
import { logUnexpected } from "../../services/errorMessage";

export interface CreateSecretFormProps {
  scope: SecretScope;
  /** The project for `project`; omitted for `global` and for "my secrets". */
  scopeId?: string;
}

export function CreateSecretForm({ scope, scopeId }: CreateSecretFormProps) {
  const queryClient = useQueryClient();
  const listKey = queryKeys.secrets.list(scope, scopeId);

  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [orchestratorOnly, setOrchestratorOnly] = useState(false);
  const [nameError, setNameError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: (body: CreateSecretRequest) => createSecret(body),
    onSuccess: (created: SecretMeta) => {
      // The new row shows before the refetch lands; the table sorts by name.
      queryClient.setQueryData<SecretMeta[]>(listKey, (rows) =>
        rows === undefined ? [created] : [created, ...rows],
      );
      void queryClient.invalidateQueries({ queryKey: listKey });
      // A credential can be created here too, by typing its name; whichever it
      // was, the agent-credential answers are stale (`SPEC.md`, "Frontend").
      invalidateSecretQueries(queryClient);
      setName("");
      setOrchestratorOnly(false);
      setError(null);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      setError(secretErrorMessage(caught));
    },
    // Whatever happened, the plaintext goes now; the user retypes it. The
    // mutation cache keeps `variables` — the body with the value — for as long
    // as the mutation lives, so that copy is dropped here too.
    gcTime: 0,
    onSettled: () => {
      setValue("");
      create.reset();
    },
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const invalid = validateSecretName(name);
    setNameError(invalid);
    if (invalid !== null) {
      return;
    }
    setError(null);
    create.mutate({
      scope,
      ...(scopeId === undefined ? {} : { scope_id: scopeId }),
      name,
      value,
      orchestrator_only: orchestratorOnly,
    });
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Add a secret"
      className="border-console-border bg-console-surface grid gap-3 rounded border p-3 sm:grid-cols-[minmax(14rem,1fr)_2fr]"
    >
      <div className="flex flex-col gap-3">
        <FormField
          label="Name"
          name="secret-name"
          value={name}
          onChange={(next) => {
            setName(next.toUpperCase());
            setNameError(null);
          }}
          error={nameError ?? undefined}
          hint={SECRET_NAME_HINT}
          autoComplete="off"
          required
        />

        <div className="flex flex-col gap-1">
          <label className="text-console-muted flex items-center gap-2 text-xs">
            <input
              type="checkbox"
              checked={orchestratorOnly}
              onChange={(event) => {
                setOrchestratorOnly(event.target.checked);
              }}
              aria-describedby="secret-orchestrator-only-hint"
              className="accent-console-accent size-3.5"
            />
            Orchestrator only
          </label>
          <p
            id="secret-orchestrator-only-hint"
            className="text-console-muted text-xs"
          >
            {ORCHESTRATOR_ONLY_HINT}
          </p>
        </div>
      </div>

      <div className="flex flex-col gap-3">
        <FieldShell
          label="Value"
          name="secret-value"
          hint="Stored encrypted and never shown again."
          required
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

        {error !== null && <Alert kind="error">{error}</Alert>}

        <div className="flex justify-end">
          <SubmitButton loading={create.isPending}>Add secret</SubmitButton>
        </div>
      </div>
    </form>
  );
}
