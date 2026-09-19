// One row of the secrets table and the panel it expands: replace the value
// (`PUT /secrets/{id}`), rename (`PATCH`), toggle orchestrator-only (`PATCH`),
// delete (`DELETE`) and the recent uses (`GET /secrets/{id}/uses`).
//
// The metadata is everything the manager can show — `SPEC.md`, "Secrets":
// "The value is never shown after entry." A replacement value lives in this
// component's state until the mutation settles and nowhere else.

import {
  ChevronDownIcon,
  ChevronRightIcon,
  LockClosedIcon,
} from "@heroicons/react/24/outline";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { queryKeys } from "../../services/queryKeys";
import {
  deleteSecret,
  patchSecret,
  replaceSecretValue,
} from "../../services/secrets";
import type { PatchSecretRequest, SecretMeta, SecretScope } from "../../types";
import { formatDateTime, formatRelative, PLACEHOLDER } from "../../utils/format";
import { validateSecretName } from "../../utils/secretName";
import { Alert } from "../Alert";
import { SubmitButton } from "../SubmitButton";
import { SecretUsesList } from "./SecretUsesList";
import { secretErrorMessage } from "./messages";

/** The project credential of `SPEC.md`, "Projects". */
const GIT_CREDENTIAL = "GIT_CREDENTIAL";

const CELL = "py-1.5 pr-3 align-middle";

type Panel = "replace" | "rename" | "uses";

export interface SecretRowProps {
  secret: SecretMeta;
  scope: SecretScope;
  scopeId?: string;
  /** Usernames for `created_by` and for a `git` use, when resolvable. */
  usernames: Map<string, string>;
}

export function SecretRow({
  secret,
  scope,
  scopeId,
  usernames,
}: SecretRowProps) {
  const queryClient = useQueryClient();
  const listKey = queryKeys.secrets.list(scope, scopeId);

  const [panel, setPanel] = useState<Panel | null>(null);
  const [value, setValue] = useState("");
  const [name, setName] = useState(secret.name);
  const [nameError, setNameError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  /** The returned metadata replaces the row before the refetch arrives. */
  function applyUpdate(updated: SecretMeta) {
    queryClient.setQueryData<SecretMeta[]>(listKey, (rows) =>
      rows?.map((row) => (row.id === updated.id ? updated : row)),
    );
    void queryClient.invalidateQueries({ queryKey: listKey });
  }

  const replace = useMutation({
    mutationFn: (next: string) => replaceSecretValue(secret.id, next),
    onSuccess: (updated) => {
      applyUpdate(updated);
      setError(null);
      // The textarea goes away with the panel.
      setPanel(null);
    },
    onError: (caught: unknown) => {
      setError(secretErrorMessage(caught));
    },
    // The mutation cache keeps `variables` — here the plaintext — for as long
    // as the mutation lives, so it is dropped the moment the request settles.
    gcTime: 0,
    onSettled: () => {
      setValue("");
      replace.reset();
    },
  });

  const patch = useMutation({
    mutationFn: (body: PatchSecretRequest) => patchSecret(secret.id, body),
    onSuccess: (updated, body) => {
      applyUpdate(updated);
      setError(null);
      // A rename closes its editor; a flag toggle has none open.
      if (body.name !== undefined) {
        setPanel(null);
      }
    },
    onError: (caught: unknown) => {
      // A duplicate name keeps the editor open so the name can be corrected.
      setError(secretErrorMessage(caught));
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteSecret(secret.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: listKey });
    },
    onError: (caught: unknown) => {
      setError(secretErrorMessage(caught));
    },
  });

  function togglePanel(next: Panel) {
    setError(null);
    setPanel((current) => (current === next ? null : next));
  }

  function onReplace(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    replace.mutate(value);
  }

  function onRename(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const invalid = validateSecretName(name);
    setNameError(invalid);
    if (invalid !== null) {
      return;
    }
    setError(null);
    patch.mutate({ name });
  }

  function onDelete() {
    if (
      !window.confirm(
        `Delete ${secret.name}? Sessions launched later will not receive it.`,
      )
    ) {
      return;
    }
    setError(null);
    remove.mutate();
  }

  const createdBy =
    secret.created_by === null
      ? PLACEHOLDER
      : (usernames.get(secret.created_by) ?? PLACEHOLDER);

  const busy = replace.isPending || patch.isPending || remove.isPending;
  const isGitCredential = scope === "project" && secret.name === GIT_CREDENTIAL;

  return (
    <>
      <tr className="border-console-border/60 border-b last:border-b-0">
        <td className={`${CELL} font-mono text-xs`}>
          <span className="text-console-text">{secret.name}</span>
          {isGitCredential && (
            <span className="text-console-muted block font-sans text-xs">
              Used by git operations for this project
            </span>
          )}
        </td>

        <td className={CELL}>
          <label
            title="Never injected into session containers"
            className="text-console-muted inline-flex items-center gap-1.5"
          >
            <input
              type="checkbox"
              checked={secret.orchestrator_only}
              disabled={busy}
              aria-label={`Orchestrator only: ${secret.name}`}
              onChange={(event) => {
                setError(null);
                patch.mutate({ orchestrator_only: event.target.checked });
              }}
              className="accent-console-accent size-3.5"
            />
            {secret.orchestrator_only && (
              <LockClosedIcon aria-hidden="true" className="size-3.5" />
            )}
          </label>
        </td>

        <td className={`${CELL} text-console-muted font-mono text-xs`}>
          v{secret.key_version}
        </td>

        <td className={`${CELL} text-console-muted hidden lg:table-cell`}>
          {createdBy}
        </td>

        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap md:table-cell`}
          title={formatDateTime(secret.created_at)}
        >
          {formatRelative(secret.created_at)}
        </td>

        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap md:table-cell`}
          title={formatDateTime(secret.updated_at)}
        >
          {formatRelative(secret.updated_at)}
        </td>

        <td
          className={`${CELL} text-console-muted font-mono text-xs whitespace-nowrap`}
          title={
            secret.last_used_at === null
              ? undefined
              : formatDateTime(secret.last_used_at)
          }
        >
          {secret.last_used_at === null
            ? "never"
            : formatRelative(secret.last_used_at)}
        </td>

        <td className={`${CELL} pr-0`}>
          <div className="flex flex-wrap justify-end gap-1.5">
            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
              disabled={busy}
              onClick={() => {
                togglePanel("replace");
              }}
            >
              Replace value
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
              disabled={busy}
              onClick={() => {
                setName(secret.name);
                setNameError(null);
                togglePanel("rename");
              }}
            >
              Rename
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
              disabled={busy}
              onClick={() => {
                togglePanel("uses");
              }}
            >
              <span className="inline-flex items-center gap-1">
                {panel === "uses" ? (
                  <ChevronDownIcon aria-hidden="true" className="size-3.5" />
                ) : (
                  <ChevronRightIcon aria-hidden="true" className="size-3.5" />
                )}
                Uses
              </span>
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="danger"
              loading={remove.isPending}
              disabled={busy}
              onClick={onDelete}
            >
              Delete
            </SubmitButton>
          </div>
        </td>
      </tr>

      {(panel !== null || error !== null) && (
        <tr className="border-console-border/60 border-b last:border-b-0">
          <td colSpan={8} className="bg-console-surface/60 px-3 py-3">
            <div className="flex flex-col gap-3">
              {error !== null && <Alert kind="error">{error}</Alert>}

              {panel === "replace" && (
                <form
                  onSubmit={onReplace}
                  aria-label={`Replace the value of ${secret.name}`}
                  className="flex flex-col gap-2"
                >
                  <label
                    htmlFor={`replace-${secret.id}`}
                    className="text-console-muted text-xs"
                  >
                    New value for {secret.name}
                  </label>
                  <textarea
                    id={`replace-${secret.id}`}
                    rows={3}
                    value={value}
                    onChange={(event) => {
                      setValue(event.target.value);
                    }}
                    autoComplete="off"
                    spellCheck={false}
                    required
                    className="border-console-border bg-console-bg text-console-text rounded border px-2.5 py-1.5 font-mono text-sm"
                  />
                  <div className="flex gap-2">
                    <SubmitButton loading={replace.isPending}>
                      Save value
                    </SubmitButton>
                    <SubmitButton
                      type="button"
                      variant="ghost"
                      loading={false}
                      onClick={() => {
                        setValue("");
                        setPanel(null);
                      }}
                    >
                      Cancel
                    </SubmitButton>
                  </div>
                </form>
              )}

              {panel === "rename" && (
                <form
                  onSubmit={onRename}
                  aria-label={`Rename ${secret.name}`}
                  className="flex flex-col gap-2"
                >
                  <label
                    htmlFor={`rename-${secret.id}`}
                    className="text-console-muted text-xs"
                  >
                    New name for {secret.name}
                  </label>
                  <input
                    id={`rename-${secret.id}`}
                    type="text"
                    value={name}
                    onChange={(event) => {
                      setName(event.target.value.toUpperCase());
                      setNameError(null);
                    }}
                    autoComplete="off"
                    spellCheck={false}
                    required
                    aria-invalid={nameError !== null ? true : undefined}
                    className="border-console-border bg-console-bg text-console-text aria-invalid:border-state-failed max-w-sm rounded border px-2.5 py-1.5 font-mono text-sm"
                  />
                  {nameError !== null && (
                    <p className="text-state-failed text-xs">{nameError}</p>
                  )}
                  <div className="flex gap-2">
                    <SubmitButton loading={patch.isPending}>
                      Save name
                    </SubmitButton>
                    <SubmitButton
                      type="button"
                      variant="ghost"
                      loading={false}
                      onClick={() => {
                        setPanel(null);
                      }}
                    >
                      Cancel
                    </SubmitButton>
                  </div>
                </form>
              )}

              {panel === "uses" && (
                <SecretUsesList secretId={secret.id} usernames={usernames} />
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
