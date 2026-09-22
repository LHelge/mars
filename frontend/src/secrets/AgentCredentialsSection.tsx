// The `Agent credentials` section at the top of `/secrets` (`SPEC.md`,
// "Frontend", Agent credentials): every credential that applies to the caller,
// under its label rather than its name, with the scope it applies to, its last
// use, and the two actions a credential has — replace the value and delete it.
// Renaming is not one of them: the name is the credential.
//
// It reads the scopes a launch resolves over — the caller's own user scope,
// each project, and global (`ARCHITECTURE.md`, "Secrets") — rather than the
// one scope the picker below has selected, because "is there a credential at
// all" is not a question about the scope someone happens to be looking at. An
// administrator who has selected another user's scope sees that scope too,
// under that user's name.

import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { Alert } from "../components/Alert";
import { EmptyState } from "../components/EmptyState";
import { FieldShell } from "../components/FieldShell";
import { CONTROL } from "../components/fieldStyles";
import { LoadingState } from "../components/LoadingState";
import { SectionHeader } from "../components/SectionHeader";
import { ConfirmPanel } from "../components/ConfirmPanel";
import { SubmitButton } from "../components/SubmitButton";
import { TableHead } from "../components/TableHead";
import {
  CELL,
  ROW,
  SPAN_CELL_ROOMY,
  TABLE,
  X_SCROLLER,
  type TableColumn,
} from "../components/tableStyles";
import { secretErrorMessage } from "../components/secrets/messages";
import { logUnexpected } from "../services/errorMessage";
import { useAuth } from "../hooks/useAuth";
import { listProjects } from "../services/projects";
import { queryKeys } from "../services/queryKeys";
import {
  deleteSecret,
  listSecrets,
  replaceSecretValue,
} from "../services/secrets";
import type { SecretMeta, SecretScope } from "../types";
import { formatDateTime, formatRelative } from "../utils/format";
import { labelForCredential } from "./agentCredentials";
import { AddAgentCredentialForm } from "./AddAgentCredentialForm";
import { invalidateSecretQueries } from "./invalidate";

const COLUMNS: readonly TableColumn[] = [
  { label: "Credential" },
  { label: "Applies to" },
  { label: "Last used" },
  { label: "Actions", className: "pr-0 text-right" },
];

/** `SPEC.md`, "Frontend": a credential is what makes a session authenticate. */
const NOTHING_HERE =
  "Sessions cannot authenticate without one: agents launched now will fail at the first request to the model.";

/** One scope the section reads, with the name its rows are listed under. */
interface ScopeView {
  scope: SecretScope;
  scopeId?: string;
  /** `You`, the project's name, `Everyone`, or another user's username. */
  appliesTo: string;
}

export interface AgentCredentialsSectionProps {
  /**
   * The user an administrator has selected in the scope picker below. Their
   * user scope joins the list, labelled with their username rather than `You`.
   */
  otherUser?: { id: string; username: string };
}

export function AgentCredentialsSection({
  otherUser,
}: AgentCredentialsSectionProps) {
  const { user } = useAuth();

  const projects = useQuery({
    queryKey: queryKeys.projects.list(),
    queryFn: listProjects,
  });

  const scopes: ScopeView[] = [
    { scope: "user", appliesTo: "You" },
    ...(projects.data ?? []).map((project): ScopeView => ({
      scope: "project",
      scopeId: project.id,
      appliesTo: project.name,
    })),
    { scope: "global", appliesTo: "Everyone" },
    ...(otherUser === undefined || otherUser.id === user?.id
      ? []
      : [
          {
            scope: "user" as const,
            scopeId: otherUser.id,
            appliesTo: otherUser.username,
          },
        ]),
  ];

  const lists = useQueries({
    queries: scopes.map((view) => ({
      queryKey: queryKeys.secrets.list(view.scope, view.scopeId),
      queryFn: () =>
        listSecrets({
          scope: view.scope,
          ...(view.scopeId === undefined ? {} : { scope_id: view.scopeId }),
        }),
    })),
  });

  // Driven by `scopes`, so each row carries the scope it came from: `lists` is
  // built from `scopes` and is the same length, and an absent query answers
  // with no rows exactly as an unread one does.
  const rows = scopes.flatMap((view, index) =>
    (lists[index]?.data ?? [])
      .filter((secret) => secret.credential_for !== null)
      .map((secret) => ({ secret, view })),
  );

  const pending = projects.isPending || lists.some((list) => list.isPending);
  // A scope the caller may not read is not an error here — the picker below
  // says so in its own place — but a real failure should not read as "none".
  const failed = lists.some((list) => list.isError);

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Agent credentials"
        description="What agents authenticate with. Every session picks up the most specific credential that applies to the user who launches it."
        help="agent-credentials"
      />

      {failed && (
        <Alert kind="error">
          Some scopes could not be read, so this list may be incomplete.
        </Alert>
      )}

      <AddAgentCredentialForm projects={projects.data ?? []} />

      {pending ? (
        <LoadingState label="Loading agent credentials" />
      ) : rows.length === 0 ? (
        <EmptyState
          tone="warning"
          title="No agent credential"
          description={NOTHING_HERE}
        />
      ) : (
        <div className={X_SCROLLER}>
          <table className={TABLE}>
            <TableHead columns={COLUMNS} />
            <tbody>
              {rows.map(({ secret, view }) => (
                <AgentCredentialRow
                  key={secret.id}
                  secret={secret}
                  appliesTo={view.appliesTo}
                  listKey={queryKeys.secrets.list(view.scope, view.scopeId)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

interface AgentCredentialRowProps {
  secret: SecretMeta;
  appliesTo: string;
  /** The list this row came out of, so a delete can take it out of it. */
  listKey: readonly unknown[];
}

function AgentCredentialRow({
  secret,
  appliesTo,
  listKey,
}: AgentCredentialRowProps) {
  const queryClient = useQueryClient();
  const label = labelForCredential(secret.name);

  const [replacing, setReplacing] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  const replace = useMutation({
    mutationFn: (next: string) => replaceSecretValue(secret.id, next),
    onSuccess: () => {
      invalidateSecretQueries(queryClient);
      setError(null);
      setReplacing(false);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      setError(secretErrorMessage(caught));
    },
    // The mutation cache keeps `variables` — the plaintext — for as long as the
    // mutation lives, so it is dropped as soon as the request settles.
    gcTime: 0,
    onSettled: () => {
      setValue("");
      replace.reset();
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteSecret(secret.id),
    onSuccess: () => {
      // The row goes with the request that deleted it rather than with the
      // refetch behind it: until then its own actions would answer 404.
      queryClient.setQueryData<SecretMeta[]>(listKey, (rows) =>
        rows?.filter((row) => row.id !== secret.id),
      );
      invalidateSecretQueries(queryClient);
    },
    onError: (caught: unknown) => {
      logUnexpected(caught);
      setError(secretErrorMessage(caught));
    },
  });

  const busy = replace.isPending || remove.isPending;

  function onReplace(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    replace.mutate(value);
  }

  function onDelete() {
    setConfirmingDelete(false);
    setError(null);
    remove.mutate();
  }

  return (
    <>
      <tr className={ROW}>
        <td className={`${CELL} text-console-text`}>{label}</td>

        <td className={`${CELL} text-console-muted`}>{appliesTo}</td>

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
              onClick={() => {
                setError(null);
                setConfirmingDelete(false);
                // Toggling the panel shut drops the typed value: a plaintext
                // must not survive a closed panel (`CLAUDE.md`, rule 3).
                setValue("");
                setReplacing((open) => !open);
              }}
            >
              Replace value
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="danger"
              loading={remove.isPending}
              disabled={busy}
              aria-expanded={confirmingDelete}
              onClick={() => {
                setError(null);
                setConfirmingDelete((open) => !open);
              }}
            >
              Delete
            </SubmitButton>
          </div>
        </td>
      </tr>

      {(replacing || confirmingDelete || error !== null) && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className={SPAN_CELL_ROOMY}>
            <div className="flex flex-col gap-3">
              {error !== null && <Alert kind="error">{error}</Alert>}

              {confirmingDelete && (
                <ConfirmPanel
                  message={`Delete the ${label} that applies to ${appliesTo}? Sessions launched later will have to authenticate some other way.`}
                  confirmLabel={`Delete the ${label} that applies to ${appliesTo}`}
                  pending={remove.isPending}
                  onConfirm={onDelete}
                  onCancel={() => {
                    setConfirmingDelete(false);
                  }}
                />
              )}

              {replacing && (
                <form
                  onSubmit={onReplace}
                  aria-label={`Replace the ${label} that applies to ${appliesTo}`}
                  className="flex flex-col gap-2"
                >
                  <FieldShell
                    label="New value"
                    name={`replace-credential-${secret.id}`}
                  >
                    {(control) => (
                      <input
                        {...control}
                        type="password"
                        value={value}
                        onChange={(event) => {
                          setValue(event.target.value);
                        }}
                        autoComplete="off"
                        spellCheck={false}
                        required
                        className={`${CONTROL} max-w-md`}
                      />
                    )}
                  </FieldShell>
                  <div className="flex gap-2">
                    <SubmitButton loading={replace.isPending}>
                      Save value
                    </SubmitButton>
                    <SubmitButton
                      type="button"
                      variant="ghost"
                      onClick={() => {
                        setValue("");
                        setReplacing(false);
                      }}
                    >
                      Cancel
                    </SubmitButton>
                  </div>
                </form>
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
