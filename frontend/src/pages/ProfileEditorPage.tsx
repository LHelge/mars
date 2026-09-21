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
import { Link } from "react-router";
import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { FormField } from "../components/FormField";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { SectionHeader } from "../components/SectionHeader";
import { SubmitButton } from "../components/SubmitButton";
import { CONTROL, FIELD } from "../components/fieldStyles";
import { useFormSubmit } from "../hooks";
import { AgentCredentialNotice } from "../secrets/AgentCredentialNotice";
import { useAgentCredential } from "../secrets/useAgentCredential";
import { ApiError } from "../services/apiClient";
import {
  createProfile,
  listProfileTemplates,
  updateProfile,
} from "../services/profiles";
import { queryKeys } from "../services/queryKeys";
import { listSecrets } from "../services/secrets";
import { listTaskStates } from "../services/taskStates";
import { parseProfileKind } from "../types";
import type { Profile, ProfileKind } from "../types";
import { PROFILE_GATED_TOOLS } from "../types";
import { SECRET_NAME_RE, validateSecretName } from "../utils/secretName";
import {
  BLANK_TEMPLATE,
  defaultInputForKind,
  idleTimeoutError,
  maxConcurrentError,
  mergeSecretOptions,
  MIN_IDLE_TIMEOUT_SECS,
  MIN_MAX_CONCURRENT,
  partialMessagesDecided,
  partialMessagesDefault,
  prefillFromTemplate,
  PROFILE_BACKEND,
  PROFILE_PERMISSION_MODE,
  toFormState,
  toggleMember,
  toInput,
  toProfileInput,
  unattendedCredential,
} from "./project/profileForm";
import type { ProfileFormState } from "./project/profileForm";

/** A value the editor shows but nobody can change: quieter, and not a field. */
const READ_ONLY_CLASS =
  "border-console-border bg-console-raised text-console-muted rounded border px-2.5 py-1.5 font-mono text-sm";

const CHECK_CLASS = "accent-console-accent size-3.5";

export interface ProfileEditorPageProps {
  projectId: string;
  /** The profile being edited; `null` creates a new one. */
  profile: Profile | null;
  /** The project's default profile image, prefilled when creating. */
  defaultImage: string;
  /**
   * The profile names already taken in this project, so a pre-fill from a
   * role template suffixes its name instead of colliding on save.
   */
  existingNames: string[];
  /** Back to the list, after a save or a cancel. */
  onClose: () => void;
}

export function ProfileEditorPage({
  projectId,
  profile,
  defaultImage,
  existingNames,
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
  // Until partial messages have been decided, the kind decides: switching to
  // `ephemeral` turns it off, switching back turns it on. A stored profile
  // whose flag already differs from its kind's default has decided, so editing
  // it and moving the kind to and fro leaves that answer alone.
  const [partialTouched, setPartialTouched] = useState(() =>
    partialMessagesDecided(profile),
  );
  const [newSecret, setNewSecret] = useState("");
  const [newSecretError, setNewSecretError] = useState<string | null>(null);
  // The server's refusal of `auto_launch`, shown at the toggle it is about.
  const [autoLaunchError, setAutoLaunchError] = useState<string | null>(null);

  /** The template the form was last filled from; `""` is `Blank`. */
  const [templateName, setTemplateName] = useState(BLANK_TEMPLATE);
  /** Served states of that template this project has no queue state for. */
  const [droppedStates, setDroppedStates] = useState<string[]>([]);
  // Whether a field has been typed since the last pre-fill, so switching
  // template asks before throwing that work away.
  const [edited, setEdited] = useState(false);

  // Only when creating: an existing profile has nothing to start from. The
  // four roles are compiled into the orchestrator, so the answer is constant
  // per build and never goes stale; a failure simply leaves `Blank`.
  const templates = useQuery({
    queryKey: queryKeys.profileTemplates.list(),
    queryFn: listProfileTemplates,
    enabled: profile === null,
    staleTime: Infinity,
  });

  const states = useQuery({
    queryKey: queryKeys.projects.taskStates(projectId),
    queryFn: () => listTaskStates(projectId),
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

  const secretScopes = [
    { label: "the shared secrets", query: globalSecrets },
    { label: "this project's secrets", query: projectSecrets },
    { label: "your own secrets", query: userSecrets },
  ];

  // A scope that answered — with its names, or with the 403 of a scope this
  // user may not list, which contributes none of its own accord. Until every
  // scope has, a declared name that is in none of them is unread rather than
  // uncreated, and is left unannotated (`SPEC.md`, "Frontend", Read failures).
  const secretsKnown = secretScopes.every(({ query }) => scopeAnswered(query));

  const queueStates = (states.data ?? []).filter(
    (state) => state.kind === "queue",
  );

  // The same answer the notice beside the secrets field already renders — one
  // query per project, shared — read here for the second question it happens
  // to settle: would this credential resolve for a launch with no user?
  const backend = profile?.backend ?? PROFILE_BACKEND;
  const { credential } = useAgentCredential(projectId, backend);
  const unattended = unattendedCredential(credential);

  const save = useFormSubmit(async () => {
    const input = toInput(form);
    try {
      if (profile === null) {
        await createProfile(projectId, input);
      } else {
        await updateProfile(projectId, profile.id, input);
      }
    } catch (caught) {
      // A declared name that turns out to be an agent credential is a 400
      // about one field, not about the form (`SPEC.md`, "Agent profiles":
      // `<NAME> is an agent credential and is injected automatically`). Shown
      // where the name was typed, and swallowed so the page does not say the
      // same thing twice.
      if (isCredentialNameError(caught)) {
        setNewSecretError(caught.error);
        return;
      }
      // So is a refusal of `auto_launch` — the kind, the cap or the missing
      // credential — which belongs at the toggle, in the server's own words.
      if (form.kind === "ephemeral" && isAutoLaunchError(caught)) {
        setAutoLaunchError(caught.error);
        return;
      }
      throw caught;
    }
    await queryClient.invalidateQueries({
      queryKey: queryKeys.projects.profiles(projectId),
    });
    onClose();
  });

  const timeoutError = idleTimeoutError(form.idle_timeout_secs);
  // The cap is sent on every kind, so it blocks the save on every kind — the
  // field itself is only shown, and only editable, on an ephemeral profile.
  const concurrentError = maxConcurrentError(form.max_concurrent);
  const nameMissing = form.name.trim() === "";
  const imageMissing = form.image.trim() === "";
  const blocked =
    timeoutError !== null ||
    concurrentError !== null ||
    nameMissing ||
    imageMissing;

  function patch(next: Partial<ProfileFormState>) {
    setEdited(true);
    setForm((current) => ({ ...current, ...next }));
  }

  /**
   * `Start from`: a role template pre-fills the form, `Blank` puts the
   * defaults back. Nothing is saved here — what lands is whatever the user
   * then submits, through the ordinary `POST`.
   */
  function onTemplateChange(next: string) {
    if (next === templateName) {
      return;
    }
    if (
      edited &&
      !window.confirm(
        "Replace what you have filled in? The template overwrites the name, served states, git tools and system prompt.",
      )
    ) {
      // The select is controlled, so declining simply re-renders the old value.
      return;
    }

    const chosen = (templates.data ?? []).find(
      (template) => template.name === next,
    );

    if (chosen === undefined) {
      setForm(toFormState(defaultInputForKind("conversational", defaultImage)));
      setDroppedStates([]);
      setTemplateName(BLANK_TEMPLATE);
    } else {
      // States are only dropped once this project's own list has arrived;
      // until then the template's are kept and the served-states fieldset
      // flags each unknown one on its own, as it does for an edited profile.
      const prefill = prefillFromTemplate(chosen, {
        defaultImage,
        existingNames,
        queueStates: states.isSuccess
          ? queueStates.map((state) => state.name)
          : null,
      });
      setForm(prefill.form);
      setDroppedStates(prefill.droppedStates);
      setTemplateName(chosen.name);
    }

    setPartialTouched(false);
    setEdited(false);
  }

  function onKindChange(kind: ProfileKind) {
    // The auto-launch controls are hidden on a conversational profile and the
    // payload drops the flag with them, so a half-typed cap must not be left
    // behind blocking a save nobody can see the reason for.
    const capStranded =
      kind !== "ephemeral" && maxConcurrentError(form.max_concurrent) !== null;
    setAutoLaunchError(null);
    patch({
      kind,
      ...(partialTouched ? {} : { partial_messages: partialMessagesDefault(kind) }),
      ...(capStranded ? { max_concurrent: String(MIN_MAX_CONCURRENT) } : {}),
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
            // Disabled while the save is out, like the footer's Cancel: a
            // click here would unmount the form mid-request and run `onClose`
            // a second time when the save landed.
            disabled={save.loading}
            onClick={onClose}
          >
            Cancel
          </SubmitButton>
        }
      />

      {/* The API phrases its own refusals — an unknown tool, a state that is
          not a queue state, a duplicate name — better than the client could. */}
      {save.error !== null && (
        <Alert kind="error" onDismiss={save.reset}>
          {save.error}
        </Alert>
      )}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
        {/* Settings */}
        <div className="flex flex-col gap-3">
          {/* Only on a new profile (`SPEC.md`, "Frontend", Role templates).
              With the request still out or failed the select holds `Blank`
              alone and the editor behaves as it did before. */}
          {profile === null && (
            <FieldShell
              label="Start from"
              name="profile-template"
              hint="A role template fills the name, served states, git tools and prompt; you can change anything before saving."
            >
              {(control) => (
                <>
                  <select
                    {...control}
                    value={templateName}
                    onChange={(event) => {
                      onTemplateChange(event.target.value);
                    }}
                    disabled={save.loading}
                    className={FIELD}
                  >
                    <option value={BLANK_TEMPLATE}>Blank</option>
                    {(templates.data ?? []).map((template) => (
                      <option key={template.name} value={template.name}>
                        {template.name}
                      </option>
                    ))}
                  </select>
                  {droppedStates.length > 0 && (
                    <p className="text-console-muted text-xs">
                      This project has no queue state called{" "}
                      <span className="text-console-text font-mono">
                        {droppedStates.join(", ")}
                      </span>
                      , so the template&rsquo;s served states were left off.
                    </p>
                  )}
                </>
              )}
            </FieldShell>
          )}

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

          <FieldShell
            label="Kind"
            name="profile-kind"
            hint={
              form.kind === "conversational"
                ? "Takes messages, parks between turns."
                : "Runs one prompt and ends."
            }
          >
            {(control) => (
              <select
                {...control}
                value={form.kind}
                onChange={(event) => {
                  // The two options below are the whole of `ProfileKind`,
                  // so an unparsed value cannot come back from this select.
                  const chosen = parseProfileKind(event.target.value);
                  if (chosen !== undefined) {
                    onKindChange(chosen);
                  }
                }}
                disabled={save.loading}
                className={FIELD}
              >
                <option value="conversational">conversational</option>
                <option value="ephemeral">ephemeral</option>
              </select>
            )}
          </FieldShell>

          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-1">
            <FieldShell
              label="Backend"
              name="profile-backend"
              hint="The only agent CLI in this version."
            >
              {(control) => (
                <output {...control} className={READ_ONLY_CLASS}>
                  {PROFILE_BACKEND}
                </output>
              )}
            </FieldShell>

            <FieldShell
              label="Permission mode"
              name="profile-permission-mode"
              hint="Sessions run in their own container, so the CLI never asks."
            >
              {(control) => (
                <output {...control} className={READ_ONLY_CLASS}>
                  {PROFILE_PERMISSION_MODE}
                </output>
              )}
            </FieldShell>
          </div>

          <FieldShell
            label="Model"
            name="profile-model"
            hint="Leave empty to let the CLI choose."
          >
            {(control) => (
              <input
                {...control}
                value={form.model}
                onChange={(event) => {
                  patch({ model: event.target.value });
                }}
                placeholder="CLI default"
                autoComplete="off"
                spellCheck={false}
                disabled={save.loading}
                className={FIELD}
              />
            )}
          </FieldShell>

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

          <FieldShell
            label="Runtime"
            name="profile-runtime"
            hint="A sandboxed runtime such as runsc or kata; empty uses the engine default."
          >
            {(control) => (
              <input
                {...control}
                value={form.runtime}
                onChange={(event) => {
                  patch({ runtime: event.target.value });
                }}
                placeholder="engine default"
                autoComplete="off"
                spellCheck={false}
                disabled={save.loading}
                className={FIELD}
              />
            )}
          </FieldShell>

          <FieldShell
            label="Idle timeout (seconds)"
            name="profile-idle-timeout"
            hint="How long a session may sit idle before it is parked."
            error={timeoutError ?? undefined}
          >
            {(control) => (
              <input
                {...control}
                type="number"
                min={MIN_IDLE_TIMEOUT_SECS}
                step={1}
                value={form.idle_timeout_secs}
                onChange={(event) => {
                  patch({ idle_timeout_secs: event.target.value });
                }}
                disabled={save.loading}
                className={FIELD}
              />
            )}
          </FieldShell>

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

          {/* Only an ephemeral profile can be launched without a person
              (`SPEC.md`, "Agent profiles"), so the controls exist only for
              one; `toInput` clears the flag for the other kind whatever the
              checkbox last held. */}
          {form.kind === "ephemeral" && (
            <fieldset className="border-console-border rounded border p-3">
              <legend className="text-console-muted px-1 text-xs">
                Unattended launches
              </legend>
              <p className="text-console-muted pb-2 text-xs">
                The dispatcher picks up tasks in the served states above and
                runs this profile on them without anyone asking. The cap holds
                it back only: your own launches are never refused by it.
              </p>

              <label className="text-console-text flex items-start gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={form.auto_launch}
                  onChange={(event) => {
                    setAutoLaunchError(null);
                    patch({ auto_launch: event.target.checked });
                  }}
                  disabled={save.loading}
                  className={`${CHECK_CLASS} mt-1`}
                />
                <span>
                  Let the dispatcher launch this profile
                  <span className="text-console-muted block text-xs">
                    Off: this profile only runs when someone launches it.
                  </span>
                </span>
              </label>

              {/* Before the save, from the answer the secrets notice below
                  already read; after it, in the server's own words. */}
              {form.auto_launch &&
                autoLaunchError === null &&
                (unattended === "missing" || unattended === "user_only") && (
                  <p className="text-state-parked pt-2 text-xs">
                    {unattended === "missing"
                      ? "No agent credential is stored for this project."
                      : "Only your own agent credential is stored."}{" "}
                    An unattended launch has no user behind it, so it needs one
                    at the project or shared scope.{" "}
                    <CredentialLink />
                  </p>
                )}

              {autoLaunchError !== null && (
                <p className="text-state-failed pt-2 text-xs">
                  {autoLaunchError}. <CredentialLink />
                </p>
              )}

              <div className="pt-3">
                <FieldShell
                  label="Live sessions of this profile"
                  name="profile-max-concurrent"
                  hint="The dispatcher waits once this many are creating or running. Every live session counts, whoever launched it."
                  error={concurrentError ?? undefined}
                >
                  {(control) => (
                    <input
                      {...control}
                      type="number"
                      min={MIN_MAX_CONCURRENT}
                      step={1}
                      value={form.max_concurrent}
                      onChange={(event) => {
                        patch({ max_concurrent: event.target.value });
                      }}
                      disabled={save.loading}
                      className={FIELD}
                    />
                  )}
                </FieldShell>
              </div>
            </fieldset>
          )}
        </div>

        {/* The prompt and the grants */}
        <div className="flex flex-col gap-3">
          <FieldShell
            label="System prompt"
            name="profile-system-prompt"
            hint="Prepended to every session of this profile."
          >
            {(control) => (
              <textarea
                {...control}
                // A role prompt is five paragraphs; short enough to scroll,
                // tall enough that the whole of one is on screen while it is
                // edited.
                rows={20}
                value={form.system_prompt}
                onChange={(event) => {
                  patch({ system_prompt: event.target.value });
                }}
                spellCheck={false}
                disabled={save.loading}
                className={`${FIELD} resize-y`}
              />
            )}
          </FieldShell>

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
            {/* A failed read of the project's states must not read as "this
                project has none" — that invites clearing a profile down to
                serving nothing (`SPEC.md`, "Frontend", Read failures). */}
            {states.isError && (
              <div className="pb-2">
                <QueryErrorAlert
                  query={states}
                  message="Could not load this project's task states."
                />
              </div>
            )}

            {queueStates.length === 0 ? (
              states.isPending ? (
                <p className="text-console-muted text-xs">Loading states…</p>
              ) : (
                states.isSuccess && (
                  <p className="text-console-muted text-xs">
                    This project has no queue states.
                  </p>
                )
              )
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
                would be a 400 on submit; show it so it can be cleared — but
                only once the project's own list has actually arrived, or every
                served state would be flagged on a cold cache. */}
            {(states.isSuccess ? form.serves_states : [])
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
              variables, from the global, project and your own scope. The
              agent&rsquo;s own credential is not one of them: it is resolved
              per launch and never declared.
            </p>

            {/* Read-only, and per caller: what *you* would launch this profile
                with (`SPEC.md`, "Frontend", Agent credentials). */}
            <div className="border-console-border/60 mb-2 border-b pb-2">
              <AgentCredentialNotice
                projectId={projectId}
                backend={profile?.backend ?? PROFILE_BACKEND}
              />
            </div>

            {/* One scope that failed leaves names out of the list below, so
                the failure is said rather than implied. */}
            {secretScopes
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

              {(secretsKnown ? form.secrets : [])
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
                className={CONTROL}
              />
              <SubmitButton
                type="button"
                variant="ghost"
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
          disabled={save.loading}
          onClick={onClose}
        >
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}

/** Where a credential that an unattended launch can resolve is stored. */
function CredentialLink() {
  return (
    <Link to="/secrets" className="underline">
      Add one under Agent credentials
    </Link>
  );
}

/**
 * A 400 about `auto_launch` (`SPEC.md`, "Agent profiles"). It is about the one
 * fieldset, so it is shown there rather than as a form error — and only while
 * that fieldset is on screen, so nothing is ever swallowed into a hidden
 * element. The credential refusal is the only one in this editor a user cannot
 * act on from the editor, hence the pointer to where a project or global
 * credential is set up.
 *
 * `max_concurrent must be at least 1` is deliberately not matched: the field
 * blocks that value before it is sent, so if the server ever says it anyway,
 * the form's own alert is where it belongs.
 */
function isAutoLaunchError(caught: unknown): caught is ApiError {
  return (
    caught instanceof ApiError &&
    caught.status === 400 &&
    caught.error.includes("auto_launch")
  );
}

/**
 * The 400 that `secrets` entries have of their own (`SPEC.md`, "Agent
 * profiles"). Matched on the sentence the API states, so any other 400 — an
 * unknown tool, a state that is not a queue state — still reaches the form's
 * own alert.
 */
function isCredentialNameError(caught: unknown): caught is ApiError {
  return (
    caught instanceof ApiError &&
    caught.status === 400 &&
    caught.error.includes("is an agent credential")
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
