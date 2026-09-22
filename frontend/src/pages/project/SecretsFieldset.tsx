// The secret names a session of this profile has injected as environment
// variables (`SPEC.md`, "Agent profiles": `secrets`).
//
// The list is every scope a session of this profile draws from, in resolution
// order (`docs/data-model.md`, `secret_scope`): global, then project, then the
// launching user. A scope the caller may not list simply contributes no names,
// and a scope that *failed* says so, because a name missing from the list
// reads as a name that does not exist. The free-text field below accepts any
// name either way — the profile declares names, and the secret behind one may
// be created later.
//
// The agent's own credential is not among them: it is resolved per launch and
// never declared, which the notice at the top says in the caller's own terms.

import { useMemo, useState } from "react";

import { QueryErrorAlert } from "../../components/QueryErrorAlert";
import { SubmitButton } from "../../components/SubmitButton";
import { CONTROL } from "../../components/fieldStyles";
import { AgentCredentialNotice } from "../../secrets/AgentCredentialNotice";
import { ApiError } from "../../services/apiClient";
import { queryKeys } from "../../services/queryKeys";
import { listSecrets } from "../../services/secrets";
import type { AgentBackend } from "../../types";
import { SECRET_NAME_RE, validateSecretName } from "../../utils/secretName";
import { useQuery } from "@tanstack/react-query";
import { CheckboxList, Fieldset } from "./profileFields";
import { mergeSecretOptions } from "./profileForm";

export interface SecretsFieldsetProps {
  projectId: string;
  /** Whose credential the notice is about. */
  backend: AgentBackend;
  /** The form's `secrets`. */
  selected: string[];
  onToggle: (name: string) => void;
  /** A name the user declared by hand, already validated and upper-cased. */
  onDeclare: (name: string) => void;
  disabled: boolean;
  /**
   * The refusal shown under the field. It is the caller's because the save
   * itself can raise one: a declared name that turns out to be an agent
   * credential is a 400 about this field, not about the form.
   */
  error: string | null;
  onErrorChange: (message: string | null) => void;
}

export function SecretsFieldset({
  projectId,
  backend,
  selected,
  onToggle,
  onDeclare,
  disabled,
  error,
  onErrorChange,
}: SecretsFieldsetProps) {
  const [typed, setTyped] = useState("");

  const globalSecrets = useSecretNames("global");
  const projectSecrets = useSecretNames("project", projectId);
  const userSecrets = useSecretNames("user");

  const options = useMemo(
    () =>
      mergeSecretOptions([
        globalSecrets.data,
        projectSecrets.data,
        userSecrets.data,
      ]),
    [globalSecrets.data, projectSecrets.data, userSecrets.data],
  );

  const scopes = [
    { label: "the shared secrets", query: globalSecrets },
    { label: "this project's secrets", query: projectSecrets },
    { label: "your own secrets", query: userSecrets },
  ];

  // A scope that answered — with its names, or with the 403 of a scope this
  // user may not list, which contributes none of its own accord. Until every
  // scope has, a declared name that is in none of them is unread rather than
  // uncreated, and is left unannotated (`SPEC.md`, "Frontend", Read failures).
  const known = scopes.every(({ query }) => scopeAnswered(query));
  const orphans = (known ? selected : []).filter(
    (name) => !options.some((option) => option.name === name),
  );

  function onAdd() {
    const name = typed.trim().toUpperCase();
    const invalid = validateSecretName(name);
    if (invalid !== null) {
      onErrorChange(invalid);
      return;
    }
    onDeclare(name);
    setTyped("");
    onErrorChange(null);
  }

  return (
    <Fieldset
      legend="Secrets"
      description="Names injected into the session container as environment variables, from the global, project and your own scope. The most specific scope wins, and if that one is orchestrator-only the name is skipped even when declared. The agent’s own credential is not one of them: it is resolved per launch and never declared."
      help="secrets"
    >
      {/* Read-only, and per caller: what *you* would launch this profile
          with (`SPEC.md`, "Frontend", Agent credentials). */}
      <div className="border-console-border/60 mb-2 border-b pb-2">
        <AgentCredentialNotice projectId={projectId} backend={backend} />
      </div>

      {/* One scope that failed leaves names out of the list below, so
          the failure is said rather than implied. */}
      {scopes
        .filter(({ query }) => query.isError && !scopeAnswered(query))
        .map(({ label, query }) => (
          <div key={label} className="pb-2">
            <QueryErrorAlert
              kind="warning"
              query={query}
              message={`Could not load ${label}; names from that scope are missing below.`}
            />
          </div>
        ))}

      <div className="flex flex-col gap-1.5">
        <CheckboxList
          items={options.map((option) => ({
            name: option.name,
            // An orchestrator-only secret cannot be added; one that is already
            // listed can still be taken off the list.
            locked: option.orchestrator_only,
            ...(option.orchestrator_only
              ? {
                  note: (
                    <span className="text-console-muted font-sans">
                      never injected
                    </span>
                  ),
                }
              : {}),
          }))}
          selected={selected}
          onToggle={onToggle}
          disabled={disabled}
        />

        <CheckboxList
          items={orphans.map((name) => ({
            name,
            note: (
              <span className="text-console-muted font-sans">
                not created yet
              </span>
            ),
          }))}
          selected={selected}
          onToggle={onToggle}
          disabled={disabled}
        />
      </div>

      <div className="flex flex-wrap items-start gap-2 pt-3">
        <input
          aria-label="Secret name to declare"
          value={typed}
          onChange={(event) => {
            setTyped(event.target.value.toUpperCase());
            onErrorChange(null);
          }}
          placeholder="ANOTHER_SECRET"
          autoComplete="off"
          spellCheck={false}
          pattern={SECRET_NAME_RE.source}
          disabled={disabled}
          className={CONTROL}
        />
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={disabled || typed.trim() === ""}
          onClick={onAdd}
        >
          Declare name
        </SubmitButton>
      </div>
      {error !== null && (
        <p className="text-state-failed pt-1.5 text-xs">{error}</p>
      )}
    </Fieldset>
  );
}

/** One scope's names; a scope the caller may not read contributes none. */
function useSecretNames(
  scope: "global" | "project" | "user",
  scopeId?: string,
) {
  return useQuery({
    queryKey: queryKeys.secrets.list(scope, scopeId),
    queryFn: () =>
      listSecrets({
        scope,
        ...(scopeId === undefined ? {} : { scope_id: scopeId }),
      }),
  });
}

/**
 * Whether a scope has given its answer: its names, or the 403 of a scope this
 * user may not list, which means it contributes none. A failed read has said
 * nothing either way.
 */
function scopeAnswered(query: { isSuccess: boolean; error: unknown }): boolean {
  return (
    query.isSuccess ||
    (query.error instanceof ApiError && query.error.status === 403)
  );
}
