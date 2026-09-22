// The guided form of `SPEC.md`, "Frontend", Agent credentials: three fields —
// kind, value, `Applies to` — that write an ordinary `POST /secrets` under the
// name the kind dictates. The name is never typed and there is no
// orchestrator-only control: a credential may not be orchestrator-only
// (ADR 0036), so offering the box would only offer a 400.
//
// The value lives in this component's state while the request is in flight and
// is dropped when it settles; it is never put in a query cache, a URL or
// `localStorage` (`CLAUDE.md`, rule 3). The field is `type="password"`, so it
// is not echoed back on screen either.

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { Alert } from "../components/Alert";
import { FormField } from "../components/FormField";
import { HelpLink } from "../components/HelpLink";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { createSecret } from "../services/secrets";
import type { Project } from "../types";
import {
  ALL_AGENT_CREDENTIALS,
  credentialScope,
  type AppliesTo,
} from "./agentCredentials";
import { invalidateSecretQueries } from "./invalidate";

const SELECT_CLASS =
  "border-console-border bg-console-bg text-console-text rounded border px-2 py-1 font-mono text-xs disabled:opacity-50";

/**
 * What `Applies to` decides (`ARCHITECTURE.md`, "Secrets", Agent credentials,
 * and "Task tracker" → "Unattended launches"): a person's launch resolves the
 * most specific of their own, the project's and the global credential, and an
 * unattended one has no user, so only the last two can ever reach it.
 */
const APPLIES_TO_HINT =
  "Your own launches use the most specific credential that applies: yours, then the project's, then Everyone's. Automatic and scheduled runs have no user behind them, so they need a project or Everyone credential.";

/** The third field, in the order it is offered; `Me` is the default. */
const APPLIES_TO: { value: AppliesTo; label: string }[] = [
  { value: "me", label: "Me" },
  { value: "project", label: "A project" },
  { value: "everyone", label: "Everyone" },
];

export interface AddAgentCredentialFormProps {
  /** The projects the `A project` option can choose between. */
  projects: Project[];
}

export function AddAgentCredentialForm({
  projects,
}: AddAgentCredentialFormProps) {
  const queryClient = useQueryClient();

  const [name, setName] = useState(ALL_AGENT_CREDENTIALS[0]?.name ?? "");
  const [value, setValue] = useState("");
  const [appliesTo, setAppliesTo] = useState<AppliesTo>("me");
  const [projectId, setProjectId] = useState<string | null>(null);

  const noProjects = projects.length === 0;
  const target = credentialScope(appliesTo, projectId);

  // `useFormSubmit` shows an `ApiError`'s `error` as the server wrote it, which
  // is what the 409 wants: it names the credential already at that scope
  // (`SPEC.md`, "Secrets").
  const { submit, loading, error } = useFormSubmit(async () => {
    if (target === null) {
      return;
    }
    try {
      await createSecret({ ...target, name, value });
    } finally {
      // Whatever happened, the plaintext goes now and the user retypes it,
      // exactly as the general create form does.
      setValue("");
    }
    invalidateSecretQueries(queryClient);
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void submit();
  }

  const kind = ALL_AGENT_CREDENTIALS.find((entry) => entry.name === name);

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Add agent credential"
      className="border-console-border bg-console-surface grid gap-4 rounded border p-3 sm:grid-cols-[minmax(14rem,1fr)_2fr]"
    >
      <div className="flex flex-col gap-4">
        <fieldset className="flex flex-col gap-1.5">
          <legend className="text-console-muted mb-1.5 text-xs">Kind</legend>
          {ALL_AGENT_CREDENTIALS.map((entry) => (
            <label
              key={entry.name}
              className="text-console-text flex items-center gap-1.5 text-sm"
            >
              <input
                type="radio"
                name="agent-credential-kind"
                value={entry.name}
                checked={name === entry.name}
                onChange={() => {
                  setName(entry.name);
                }}
                className="accent-console-accent size-3.5"
              />
              {entry.label}
            </label>
          ))}
          {kind?.hint !== undefined && (
            <p className="text-console-muted max-w-prose text-xs">
              {kind.hint}
            </p>
          )}
        </fieldset>

        <fieldset
          className="flex flex-col gap-1.5"
          aria-describedby="agent-credential-applies-to-hint"
        >
          <legend className="text-console-muted mb-1.5 text-xs">
            Applies to
          </legend>
          {APPLIES_TO.map((entry) => {
            const disabled = entry.value === "project" && noProjects;
            return (
              <label
                key={entry.value}
                className={`flex items-center gap-1.5 text-sm ${
                  disabled ? "text-console-muted" : "text-console-text"
                }`}
              >
                <input
                  type="radio"
                  name="agent-credential-applies-to"
                  value={entry.value}
                  checked={appliesTo === entry.value}
                  disabled={disabled}
                  onChange={() => {
                    setAppliesTo(entry.value);
                  }}
                  className="accent-console-accent size-3.5"
                />
                {entry.label}
              </label>
            );
          })}

          {noProjects ? (
            <p className="text-console-muted text-xs">
              No projects yet, so a credential cannot be given to one.
            </p>
          ) : (
            appliesTo === "project" && (
              <select
                aria-label="Project"
                value={projectId ?? ""}
                onChange={(event) => {
                  setProjectId(event.target.value);
                }}
                className={`${SELECT_CLASS} mt-1 self-start`}
              >
                <option value="" disabled>
                  Choose a project
                </option>
                {projects.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
            )
          )}

          {/* The link sits beside the hint, outside what the fieldset's
              `aria-describedby` names (`SPEC.md`, "Frontend", Help). */}
          <div className="text-console-muted max-w-prose text-xs">
            <p id="agent-credential-applies-to-hint" className="inline">
              {APPLIES_TO_HINT}
            </p>{" "}
            <HelpLink topic="agent-credentials" />
          </div>
        </fieldset>
      </div>

      <div className="flex flex-col gap-3">
        <FormField
          label="Value"
          name="agent-credential-value"
          type="password"
          value={value}
          onChange={setValue}
          hint="Stored encrypted and never shown again."
          autoComplete="off"
          required
        />

        {error !== null && <Alert kind="error">{error}</Alert>}

        <div className="flex justify-end">
          <SubmitButton loading={loading} disabled={target === null}>
            Add agent credential
          </SubmitButton>
        </div>
      </div>
    </form>
  );
}
