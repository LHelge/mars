// The agent-profile editor of `SPEC.md`, "Frontend", Pages: one form that
// creates (`POST /projects/{pid}/profiles`) or replaces (`PUT .../{id}`) a
// profile. It is mounted inside the project page's profiles tab rather than on
// a route of its own, and the tab drives it through `?profile=new|<id>`, so
// an open editor is still a link somebody can send.
//
// `PUT` replaces the whole profile, so the form always submits every field.
// Two fields are fixed in v1 and shown read-only rather than hidden, because
// an agent's backend and permission mode are things an operator wants to see:
// `backend` is `claude` and `permission_mode` is `bypass`.
//
// Layout: settings on the left, the system prompt on the right, because the
// prompt is the field people actually write in and it wants the height.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import {
  Alert,
  FormField,
  SectionHeader,
  SubmitButton,
} from "../components";
import { useFormSubmit } from "../hooks";
import { createProfile, updateProfile } from "../services/profiles";
import { queryKeys } from "../services/queryKeys";
import { listSecrets } from "../services/secrets";
import { listTaskStates } from "../services/taskStates";
import type { Profile, ProfileKind, SecretMeta } from "../types";
import { PROFILE_GATED_TOOLS } from "../types";
import { SECRET_NAME_RE, validateSecretName } from "../utils/secretName";
import {
  defaultInputForKind,
  idleTimeoutError,
  MIN_IDLE_TIMEOUT_SECS,
  partialMessagesDefault,
  PROFILE_BACKEND,
  PROFILE_PERMISSION_MODE,
  toFormState,
  toggleMember,
  toInput,
  toProfileInput,
} from "./project/profileForm";
import type { ProfileFormState } from "./project/profileForm";

const INPUT_CLASS =
  "border-console-border bg-console-bg text-console-text placeholder:text-console-muted rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50";

const READ_ONLY_CLASS =
  "border-console-border bg-console-raised text-console-muted rounded border px-2.5 py-1.5 font-mono text-sm";

const CHECK_CLASS = "accent-console-accent size-3.5";

export interface ProfileEditorPageProps {
  projectId: string;
  /** The profile being edited; `null` creates a new one. */
  profile: Profile | null;
  /** The project's default profile image, prefilled when creating. */
  defaultImage: string;
  /** Back to the list, after a save or a cancel. */
  onClose: () => void;
}

export function ProfileEditorPage({
  projectId,
  profile,
  defaultImage,
  onClose,
}: ProfileEditorPageProps) {
  const queryClient = useQueryClient();

  const [form, setForm] = useState<ProfileFormState>(() =>
    toFormState(
      profile === null
        ? defaultInputForKind("conversational", defaultImage)
        : toProfileInput(profile),
    ),
  );
  // Until the user has decided about partial messages themselves, the kind
  // decides: switching to `ephemeral` turns it off, switching back turns it on.
  const [partialTouched, setPartialTouched] = useState(false);
  const [newSecret, setNewSecret] = useState("");
  const [newSecretError, setNewSecretError] = useState<string | null>(null);

  const states = useQuery({
    queryKey: queryKeys.projects.taskStates(projectId),
    queryFn: () => listTaskStates(projectId),
    retry: false,
  });

  // Every scope a session of this profile draws from, in resolution order
  // (`docs/data-model.md`, `secret_scope`): global, then project, then the
  // launching user. A scope the caller may not list simply contributes no
  // names; the free-text field below still accepts any of them.
  const globalSecrets = useSecretNames("global");
  const projectSecrets = useSecretNames("project", projectId);
  const userSecrets = useSecretNames("user");

  const secretOptions = useMemo(
    () =>
      mergeSecretOptions([
        globalSecrets.data,
        projectSecrets.data,
        userSecrets.data,
      ]),
    [globalSecrets.data, projectSecrets.data, userSecrets.data],
  );

  const queueStates = (states.data ?? []).filter(
    (state) => state.kind === "queue",
  );

  const save = useFormSubmit(async () => {
    const input = toInput(form);
    if (profile === null) {
      await createProfile(projectId, input);
    } else {
      await updateProfile(projectId, profile.id, input);
    }
    await queryClient.invalidateQueries({
      queryKey: queryKeys.projects.profiles(projectId),
    });
    onClose();
  });

  const timeoutError = idleTimeoutError(form.idle_timeout_secs);
  const nameMissing = form.name.trim() === "";
  const imageMissing = form.image.trim() === "";
  const blocked = timeoutError !== null || nameMissing || imageMissing;

  function patch(next: Partial<ProfileFormState>) {
    setForm((current) => ({ ...current, ...next }));
  }

  function onKindChange(kind: ProfileKind) {
    patch({
      kind,
      ...(partialTouched ? {} : { partial_messages: partialMessagesDefault(kind) }),
    });
  }

  function onAddSecret() {
    const name = newSecret.trim().toUpperCase();
    const invalid = validateSecretName(name);
    if (invalid !== null) {
      setNewSecretError(invalid);
      return;
    }
    if (!form.secrets.includes(name)) {
      patch({ secrets: [...form.secrets, name] });
    }
    setNewSecret("");
    setNewSecretError(null);
  }

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (blocked) {
      return;
    }
    void save.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label={profile === null ? "New profile" : `Edit ${profile.name}`}
      className="space-y-3"
    >
      <SectionHeader
        title={profile === null ? "New profile" : `Edit ${profile.name}`}
        description="The served states and the system prompt together define what this agent does."
        actions={
          <SubmitButton
            type="button"
            variant="ghost"
            loading={false}
            onClick={onClose}
          >
            Cancel
          </SubmitButton>
        }
      />

      {/* The API phrases its own refusals — an unknown tool, a state that is
          not a queue state, a duplicate name — better than the client could. */}
      {save.error !== null && (
        <Alert kind="error" onDismiss={save.clearError}>
          {save.error}
        </Alert>
      )}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
        {/* Settings */}
        <div className="flex flex-col gap-3">
          <FormField
            label="Name"
            name="profile-name"
            value={form.name}
            onChange={(next) => {
              patch({ name: next });
            }}
            hint="Unique within the project."
            autoComplete="off"
            required
            disabled={save.loading}
          />

          <FormField
            label="Kind"
            name="profile-kind"
            value={form.kind}
            onChange={() => {
              /* handled by the select below */
            }}
            hint={
              form.kind === "conversational"
                ? "Takes messages, parks between turns."
                : "Runs one prompt and ends."
            }
          >
            <select
              id="profile-kind"
              name="profile-kind"
              value={form.kind}
              onChange={(event) => {
                onKindChange(event.target.value as ProfileKind);
              }}
              disabled={save.loading}
              className={`${INPUT_CLASS} w-full`}
            >
              <option value="conversational">conversational</option>
              <option value="ephemeral">ephemeral</option>
            </select>
          </FormField>

          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-1">
            <FormField
              label="Backend"
              name="profile-backend"
              value={PROFILE_BACKEND}
              onChange={() => {
                /* fixed in v1 */
              }}
              hint="The only agent CLI in this version."
            >
              <output id="profile-backend" className={READ_ONLY_CLASS}>
                {PROFILE_BACKEND}
              </output>
            </FormField>

            <FormField
              label="Permission mode"
              name="profile-permission-mode"
              value={PROFILE_PERMISSION_MODE}
              onChange={() => {
                /* fixed in v1 */
              }}
              hint="Sessions run in their own container, so the CLI never asks."
            >
              <output id="profile-permission-mode" className={READ_ONLY_CLASS}>
                {PROFILE_PERMISSION_MODE}
              </output>
            </FormField>
          </div>

          <FormField
            label="Model"
            name="profile-model"
            value={form.model}
            onChange={(next) => {
              patch({ model: next });
            }}
            hint="Leave empty to let the CLI choose."
            autoComplete="off"
            disabled={save.loading}
          >
            <input
              id="profile-model"
              name="profile-model"
              value={form.model}
              onChange={(event) => {
                patch({ model: event.target.value });
              }}
              placeholder="CLI default"
              autoComplete="off"
              spellCheck={false}
              disabled={save.loading}
              className={`${INPUT_CLASS} w-full`}
            />
          </FormField>

          <FormField
            label="Image"
            name="profile-image"
            value={form.image}
            onChange={(next) => {
              patch({ image: next });
            }}
            hint="The container image sessions of this profile run in."
            autoComplete="off"
            required
            disabled={save.loading}
          />

          <FormField
            label="Runtime"
            name="profile-runtime"
            value={form.runtime}
            onChange={(next) => {
              patch({ runtime: next });
            }}
            hint="A sandboxed runtime such as runsc or kata; empty uses the engine default."
            disabled={save.loading}
          >
            <input
              id="profile-runtime"
              name="profile-runtime"
              value={form.runtime}
              onChange={(event) => {
                patch({ runtime: event.target.value });
              }}
              placeholder="engine default"
              autoComplete="off"
              spellCheck={false}
              disabled={save.loading}
              className={`${INPUT_CLASS} w-full`}
            />
          </FormField>

          <FormField
            label="Idle timeout (seconds)"
            name="profile-idle-timeout"
            value={form.idle_timeout_secs}
            onChange={(next) => {
              patch({ idle_timeout_secs: next });
            }}
            hint="How long a session may sit idle before it is parked."
            {...(timeoutError === null ? {} : { error: timeoutError })}
            disabled={save.loading}
          >
            <input
              id="profile-idle-timeout"
              name="profile-idle-timeout"
              type="number"
              min={MIN_IDLE_TIMEOUT_SECS}
              step={1}
              value={form.idle_timeout_secs}
              onChange={(event) => {
                patch({ idle_timeout_secs: event.target.value });
              }}
              disabled={save.loading}
              aria-invalid={timeoutError === null ? undefined : true}
              className={`${INPUT_CLASS} aria-invalid:border-state-failed w-full`}
            />
          </FormField>

          <label className="text-console-text flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={form.partial_messages}
              onChange={(event) => {
                setPartialTouched(true);
                patch({ partial_messages: event.target.checked });
              }}
              disabled={save.loading}
              className={`${CHECK_CLASS} mt-1`}
            />
            <span>
              Stream partial messages
              <span className="text-console-muted block text-xs">
                Show the agent&rsquo;s text as it is written, not only when a
                turn finishes.
              </span>
            </span>
          </label>
        </div>

        {/* The prompt and the grants */}
        <div className="flex flex-col gap-3">
          <FormField
            label="System prompt"
            name="profile-system-prompt"
            value={form.system_prompt}
            onChange={(next) => {
              patch({ system_prompt: next });
            }}
            hint="Prepended to every session of this profile."
            disabled={save.loading}
          >
            <textarea
              id="profile-system-prompt"
              name="profile-system-prompt"
              rows={14}
              value={form.system_prompt}
              onChange={(event) => {
                patch({ system_prompt: event.target.value });
              }}
              spellCheck={false}
              disabled={save.loading}
              className={`${INPUT_CLASS} w-full resize-y`}
            />
          </FormField>

          <fieldset className="border-console-border rounded border p-3">
            <legend className="text-console-muted px-1 text-xs">
              Git tools
            </legend>
            <p className="text-console-muted pb-2 text-xs">
              The task tracker tools are always available. These four reach the
              project&rsquo;s git mirror.
            </p>
            <div className="flex flex-wrap gap-x-4 gap-y-2">
              {PROFILE_GATED_TOOLS.map((tool) => (
                <label
                  key={tool}
                  className="text-console-text flex items-center gap-2 font-mono text-xs"
                >
                  <input
                    type="checkbox"
                    checked={form.mcp_tools.includes(tool)}
                    onChange={() => {
                      patch({ mcp_tools: toggleMember(form.mcp_tools, tool) });
                    }}
                    disabled={save.loading}
                    className={CHECK_CLASS}
                  />
                  {tool}
                </label>
              ))}
            </div>
          </fieldset>

          <fieldset className="border-console-border rounded border p-3">
            <legend className="text-console-muted px-1 text-xs">
              Served states
            </legend>
            <p className="text-console-muted pb-2 text-xs">
              Queue states this profile picks work up from.
            </p>
            {queueStates.length === 0 ? (
              <p className="text-console-muted text-xs">
                {states.isPending
                  ? "Loading states…"
                  : "This project has no queue states."}
              </p>
            ) : (
              <div className="flex flex-wrap gap-x-4 gap-y-2">
                {queueStates.map((state) => (
                  <label
                    key={state.id}
                    className="text-console-text flex items-center gap-2 font-mono text-xs"
                  >
                    <input
                      type="checkbox"
                      checked={form.serves_states.includes(state.name)}
                      onChange={() => {
                        patch({
                          serves_states: toggleMember(
                            form.serves_states,
                            state.name,
                          ),
                        });
                      }}
                      disabled={save.loading}
                      className={CHECK_CLASS}
                    />
                    {state.name}
                  </label>
                ))}
              </div>
            )}
            {/* A state that was renamed or deleted since the profile was saved
                would be a 400 on submit; show it so it can be cleared. */}
            {form.serves_states
              .filter((name) => !queueStates.some((s) => s.name === name))
              .map((name) => (
                <label
                  key={name}
                  className="text-state-parked flex items-center gap-2 pt-2 font-mono text-xs"
                >
                  <input
                    type="checkbox"
                    checked
                    onChange={() => {
                      patch({
                        serves_states: toggleMember(form.serves_states, name),
                      });
                    }}
                    disabled={save.loading}
                    className={CHECK_CLASS}
                  />
                  {name}
                  <span className="font-sans">not a queue state</span>
                </label>
              ))}
          </fieldset>

          <fieldset className="border-console-border rounded border p-3">
            <legend className="text-console-muted px-1 text-xs">Secrets</legend>
            <p className="text-console-muted pb-2 text-xs">
              Names injected into the session container as environment
              variables, from the global, project and your own scope.
            </p>

            <div className="flex flex-col gap-1.5">
              {secretOptions.map((option) => {
                const checked = form.secrets.includes(option.name);
                return (
                  <label
                    key={option.name}
                    className="text-console-text flex items-center gap-2 font-mono text-xs"
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => {
                        patch({
                          secrets: toggleMember(form.secrets, option.name),
                        });
                      }}
                      // An orchestrator-only secret cannot be added; one that
                      // is already listed can still be taken off the list.
                      disabled={
                        save.loading || (option.orchestrator_only && !checked)
                      }
                      className={CHECK_CLASS}
                    />
                    {option.name}
                    {option.orchestrator_only && (
                      <span className="text-console-muted font-sans">
                        never injected
                      </span>
                    )}
                  </label>
                );
              })}

              {form.secrets
                .filter((name) => !secretOptions.some((o) => o.name === name))
                .map((name) => (
                  <label
                    key={name}
                    className="text-console-text flex items-center gap-2 font-mono text-xs"
                  >
                    <input
                      type="checkbox"
                      checked
                      onChange={() => {
                        patch({ secrets: toggleMember(form.secrets, name) });
                      }}
                      disabled={save.loading}
                      className={CHECK_CLASS}
                    />
                    {name}
                    <span className="text-console-muted font-sans">
                      not created yet
                    </span>
                  </label>
                ))}
            </div>

            <div className="flex flex-wrap items-start gap-2 pt-3">
              <input
                aria-label="Secret name to declare"
                value={newSecret}
                onChange={(event) => {
                  setNewSecret(event.target.value.toUpperCase());
                  setNewSecretError(null);
                }}
                placeholder="ANOTHER_SECRET"
                autoComplete="off"
                spellCheck={false}
                pattern={SECRET_NAME_RE.source}
                disabled={save.loading}
                className={INPUT_CLASS}
              />
              <SubmitButton
                type="button"
                variant="ghost"
                loading={false}
                disabled={save.loading || newSecret.trim() === ""}
                onClick={onAddSecret}
              >
                Declare name
              </SubmitButton>
            </div>
            {newSecretError !== null && (
              <p className="text-state-failed pt-1.5 text-xs">
                {newSecretError}
              </p>
            )}
          </fieldset>
        </div>
      </div>

      <div className="flex items-center gap-2">
        <SubmitButton loading={save.loading} disabled={blocked}>
          {profile === null ? "Create profile" : "Save profile"}
        </SubmitButton>
        <SubmitButton
          type="button"
          variant="ghost"
          loading={false}
          disabled={save.loading}
          onClick={onClose}
        >
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}

/** One scope's names; a scope the caller may not read contributes none. */
function useSecretNames(scope: "global" | "project" | "user", scopeId?: string) {
  return useQuery({
    queryKey: queryKeys.secrets.list(scope, scopeId),
    queryFn: () =>
      listSecrets({
        scope,
        ...(scopeId === undefined ? {} : { scope_id: scopeId }),
      }),
    retry: false,
  });
}

interface SecretOption {
  name: string;
  orchestrator_only: boolean;
}

/**
 * The three scopes as one list of names, sorted. A name defined in more than
 * one scope appears once; the later scope wins, which is the order sessions
 * resolve them in (`docs/data-model.md`, `secret_scope`).
 */
function mergeSecretOptions(
  lists: (SecretMeta[] | undefined)[],
): SecretOption[] {
  const byName = new Map<string, SecretOption>();
  for (const list of lists) {
    for (const meta of list ?? []) {
      byName.set(meta.name, {
        name: meta.name,
        orchestrator_only: meta.orchestrator_only,
      });
    }
  }
  return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
}
