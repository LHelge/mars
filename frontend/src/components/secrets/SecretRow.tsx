// One row of the secrets table and the panel it expands: replace the value
// (`PUT /secrets/{id}`), rename (`PATCH`), toggle orchestrator-only (`PATCH`),
// delete (`DELETE`) and the recent uses (`GET /secrets/{id}/uses`).
//
// The metadata is everything the manager can show — `SPEC.md`, "Secrets":
// "The value is never shown after entry." A replacement value lives in
// `SecretReplaceForm`'s state and nowhere else, and that form is mounted only
// while its panel is the open one, so closing the panel — by any route — is
// what drops the plaintext (`CLAUDE.md`, rule 3).

import {
  ChevronDownIcon,
  ChevronRightIcon,
  LockClosedIcon,
} from "@heroicons/react/24/outline";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { invalidateSecretQueries } from "../../secrets/invalidate";
import { logUnexpected } from "../../services/errorMessage";
import { queryKeys } from "../../services/queryKeys";
import { deleteSecret, patchSecret } from "../../services/secrets";
import type { SecretMeta, SecretScope } from "../../types";
import {
  formatDateTime,
  formatRelative,
  PLACEHOLDER,
} from "../../utils/format";
import { Alert } from "../Alert";
import { ConfirmPanel } from "../ConfirmPanel";
import { SubmitButton } from "../SubmitButton";
import { CELL, ROW, SPAN_CELL_ROOMY } from "../tableStyles";
import { SECRET_COLUMNS } from "./columns";
import { SecretRenameForm } from "./SecretRenameForm";
import { SecretReplaceForm } from "./SecretReplaceForm";
import { SecretUsesList } from "./SecretUsesList";
import { secretErrorMessage } from "./messages";

/** The project credential of `SPEC.md`, "Projects". */
const GIT_CREDENTIAL = "GIT_CREDENTIAL";

type Panel = "replace" | "rename" | "uses" | "delete";

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
  const [panelPending, setPanelPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /** The returned metadata replaces the row before the refetch arrives. */
  function applyUpdate(updated: SecretMeta) {
    queryClient.setQueryData<SecretMeta[]>(listKey, (rows) =>
      rows?.map((row) => (row.id === updated.id ? updated : row)),
    );
    void queryClient.invalidateQueries({ queryKey: listKey });
    // A rename can make a row a credential or stop it being one, so every
    // agent-credential answer is stale (`SPEC.md`, "Frontend").
    invalidateSecretQueries(queryClient);
  }

  /** Closing unmounts the panel's form, which is what drops its draft. */
  function closePanel() {
    setPanel(null);
    // The form that was reporting it is gone; nothing is in flight here.
    setPanelPending(false);
  }

  /** A panel form saved: apply the new metadata and close the panel. */
  function onSaved(updated: SecretMeta) {
    applyUpdate(updated);
    setError(null);
    closePanel();
  }

  const flag = useMutation({
    mutationFn: (next: boolean) =>
      patchSecret(secret.id, { orchestrator_only: next }),
    onSuccess: (updated) => {
      applyUpdate(updated);
      setError(null);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      setError(secretErrorMessage(caught));
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteSecret(secret.id),
    onSuccess: () => {
      // The row goes now rather than when the refetch lands: until it does,
      // every action on it would answer 404.
      queryClient.setQueryData<SecretMeta[]>(listKey, (rows) =>
        rows?.filter((row) => row.id !== secret.id),
      );
      void queryClient.invalidateQueries({ queryKey: listKey });
      invalidateSecretQueries(queryClient);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      setError(secretErrorMessage(caught));
    },
  });

  function togglePanel(next: Panel) {
    setError(null);
    setPanelPending(false);
    setPanel((current) => (current === next ? null : next));
  }

  function onDelete() {
    setError(null);
    closePanel();
    remove.mutate();
  }

  const createdBy =
    secret.created_by === null
      ? PLACEHOLDER
      : (usernames.get(secret.created_by) ?? PLACEHOLDER);

  const busy = panelPending || flag.isPending || remove.isPending;
  const isGitCredential = scope === "project" && secret.name === GIT_CREDENTIAL;

  return (
    <>
      <tr className={ROW}>
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
                flag.mutate(event.target.checked);
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
              disabled={busy}
              aria-expanded={panel === "replace"}
              onClick={() => {
                togglePanel("replace");
              }}
            >
              Replace value
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={busy}
              aria-expanded={panel === "rename"}
              onClick={() => {
                togglePanel("rename");
              }}
            >
              Rename
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="ghost"
              disabled={busy}
              aria-expanded={panel === "uses"}
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
              aria-expanded={panel === "delete"}
              onClick={() => {
                togglePanel("delete");
              }}
            >
              Delete
            </SubmitButton>
          </div>
        </td>
      </tr>

      {(panel !== null || error !== null) && (
        <tr className={ROW}>
          <td colSpan={SECRET_COLUMNS.length} className={SPAN_CELL_ROOMY}>
            <div className="flex flex-col gap-3">
              {error !== null && <Alert kind="error">{error}</Alert>}

              {panel === "replace" && (
                <SecretReplaceForm
                  secret={secret}
                  onSaved={onSaved}
                  onError={setError}
                  onPendingChange={setPanelPending}
                  onCancel={closePanel}
                />
              )}

              {panel === "rename" && (
                <SecretRenameForm
                  secret={secret}
                  onSaved={onSaved}
                  onError={setError}
                  onPendingChange={setPanelPending}
                  onCancel={closePanel}
                />
              )}

              {panel === "uses" && (
                <SecretUsesList secretId={secret.id} usernames={usernames} />
              )}

              {panel === "delete" && (
                <ConfirmPanel
                  message={`Delete ${secret.name}? Sessions launched later will not receive it; sessions already running keep the value they started with.`}
                  confirmLabel={`Delete ${secret.name}`}
                  pending={remove.isPending}
                  onConfirm={onDelete}
                  onCancel={closePanel}
                />
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
