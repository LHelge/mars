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
import { SubmitButton } from "../components/SubmitButton";
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

const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";
const CELL = "py-1.5 pr-3 align-middle";

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

  const rows = lists.flatMap((list, index) =>
    (list.data ?? [])
      .filter((secret) => secret.credential_for !== null)
      .map((secret) => ({ secret, view: scopes[index] })),
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
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  Credential
                </th>
                <th scope="col" className={HEAD}>
                  Applies to
                </th>
                <th scope="col" className={HEAD}>
                  Last used
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map(({ secret, view }) => (
                <AgentCredentialRow
                  key={secret.id}
                  secret={secret}
                  appliesTo={view.appliesTo}
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
}

function AgentCredentialRow({ secret, appliesTo }: AgentCredentialRowProps) {
  const queryClient = useQueryClient();
  const label = labelForCredential(secret.name);

  const [replacing, setReplacing] = useState(false);
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
    if (
      !window.confirm(
        `Delete the ${label} that applies to ${appliesTo}? Sessions launched later will have to authenticate some other way.`,
      )
    ) {
      return;
    }
    setError(null);
    remove.mutate();
  }

  return (
    <>
      <tr className="border-console-border/60 border-b last:border-b-0">
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
              onClick={onDelete}
            >
              Delete
            </SubmitButton>
          </div>
        </td>
      </tr>

      {(replacing || error !== null) && (
        <tr className="border-console-border/60 border-b last:border-b-0">
          <td colSpan={4} className="bg-console-surface/60 px-3 py-3">
            <div className="flex flex-col gap-3">
              {error !== null && <Alert kind="error">{error}</Alert>}

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
