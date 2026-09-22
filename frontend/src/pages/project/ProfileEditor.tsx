// The agent-profile editor of `SPEC.md`, "Frontend", Pages: one form that
// creates (`POST /projects/{pid}/profiles`) or replaces (`PUT .../{id}`) a
// profile. It is not a route: it is a panel of the profiles tab, which drives
// it through `?profile=new|<id>`, so an open editor is still a link somebody
// can send. It lives here, beside that tab and beside `profileForm.ts`, for
// the same reason.
//
// `PUT` replaces the whole profile, so the form always submits every field.
// Two fields are fixed in v1 and shown read-only rather than hidden, because
// an agent's backend and permission mode are things an operator wants to see:
// `backend` is `claude` and `permission_mode` is `bypass`.
//
// Two fieldsets exist only while the form's kind is `ephemeral`, because only
// that kind can be launched with nobody behind it: unattended launches and the
// schedule. Neither carries a rule of its own — the cron expression is judged
// by the server alone (ADR 0043) and the credential answer is the one the
// secrets notice already read.
//
// Layout: settings on the left, the system prompt on the right, because the
// prompt is the field people actually write in and it wants the height. The
// three grants below the prompt — git tools, served states, secrets — are
// fieldsets of their own, and the two that read something of the project's
// read it themselves.

import { useQueryClient } from "@tanstack/react-query";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";
import { Link } from "react-router";
import { Alert } from "../../components/Alert";
import { ConfirmPanel } from "../../components/ConfirmPanel";
import { FieldShell } from "../../components/FieldShell";
import { FormField } from "../../components/FormField";
import { SectionHeader } from "../../components/SectionHeader";
import { SubmitButton } from "../../components/SubmitButton";
import { FIELD } from "../../components/fieldStyles";
import { useFormSubmit } from "../../hooks";
import { useAgentCredential } from "../../secrets/useAgentCredential";
import { ApiError } from "../../services/apiClient";
import {
  createProfile,
  listProfileTemplates,
  updateProfile,
} from "../../services/profiles";
import { queryKeys } from "../../services/queryKeys";
import { parseProfileKind } from "../../types";
import type { Profile, ProfileKind } from "../../types";
import { PROFILE_GATED_TOOLS } from "../../types";
import { CheckboxList, Fieldset } from "./profileFields";
import { formatDateTime, formatUtc, PLACEHOLDER } from "../../utils/format";
import {
  BLANK_TEMPLATE,
  CHECK_CLASS,
  CRON_EXAMPLES,
  defaultInputForKind,
  idleTimeoutError,
  maxConcurrentError,
  MIN_IDLE_TIMEOUT_SECS,
  MIN_MAX_CONCURRENT,
  partialMessagesDecided,
  partialMessagesDefault,
  prefillFromTemplate,
  PROFILE_BACKEND,
  PROFILE_PERMISSION_MODE,
  READ_ONLY_CLASS,
  scheduleErrorField,
  scheduleErrors,
  toFormState,
  toggleMember,
  toInput,
  toProfileInput,
  unattendedCredential,
} from "./profileForm";
import type {
  ProfileFormState,
  ScheduleField,
  UnattendedCredential,
} from "./profileForm";
import { useQueueStates } from "./queueStates";
import { SecretsFieldset } from "./SecretsFieldset";
import { ServedStatesFieldset } from "./ServedStatesFieldset";

export interface ProfileEditorProps {
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

export function ProfileEditor({
  projectId,
  profile,
  defaultImage,
  existingNames,
  onClose,
}: ProfileEditorProps) {
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
  // The secrets fieldset's declare-name refusal, which the save path also
  // writes: the editor is what catches the API's answer.
  const [secretNameError, setSecretNameError] = useState<string | null>(null);
  // The server's refusal of `auto_launch`, shown at the toggle it is about.
  const [autoLaunchError, setAutoLaunchError] = useState<string | null>(null);
  // The server's refusal of the schedule, and which of its two fields it is
  // about. Only the API judges a cron expression, so this is where the reason
  // an expression was refused comes from.
  const [scheduleError, setScheduleError] = useState<{
    field: ScheduleField;
    message: string;
  } | null>(null);

  /** The template the form was last filled from; `""` is `Blank`. */
  const [templateName, setTemplateName] = useState(BLANK_TEMPLATE);
  /** A template chosen over a form the user has edited, awaiting an answer. */
  const [pendingTemplate, setPendingTemplate] = useState<string | null>(null);
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

  // The served-states fieldset renders this read; the pre-fill below needs it
  // to know which of a template's states this project cannot serve.
  const queueStates = useQueueStates(projectId);

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
        setSecretNameError(caught.error);
        return;
      }
      // So is a refusal of `auto_launch` — the kind, the cap or the missing
      // credential — which belongs at the toggle, in the server's own words.
      if (form.kind === "ephemeral" && isAutoLaunchError(caught)) {
        setAutoLaunchError(caught.error);
        return;
      }
      // And so is a refusal of the schedule, which lands on the one of its two
      // fields the server's own wording names.
      if (
        form.kind === "ephemeral" &&
        caught instanceof ApiError &&
        caught.status === 400
      ) {
        const field = scheduleErrorField(caught.error);
        if (field !== null) {
          setScheduleError({ field, message: caught.error });
          return;
        }
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
  // The schedule only exists on an ephemeral profile, and `toInput` drops it
  // for the other kind, so a half-filled pair left behind by a kind switch
  // blocks nothing.
  const schedule =
    form.kind === "ephemeral"
      ? scheduleErrors(form)
      : { cron: null, prompt: null };
  const blocked =
    timeoutError !== null ||
    concurrentError !== null ||
    schedule.cron !== null ||
    schedule.prompt !== null ||
    nameMissing ||
    imageMissing;

  /** The one message a schedule field shows: its own check, then the server's. */
  function scheduleFieldError(field: ScheduleField): string | undefined {
    const local = field === "cron" ? schedule.cron : schedule.prompt;
    if (local !== null) {
      return local;
    }
    return scheduleError?.field === field ? scheduleError.message : undefined;
  }

  /** Editing either schedule field drops an answer about what it used to say. */
  function patchSchedule(next: Partial<ProfileFormState>) {
    setScheduleError(null);
    patch(next);
  }

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
    if (edited) {
      // Ask below the select rather than over the page. The select is
      // controlled, so it goes on showing the template in force until the
      // answer arrives — and declining simply drops the pending choice.
      setPendingTemplate(next);
      return;
    }
    applyTemplate(next);
  }

  /** The chosen template, once there is nothing left to ask about. */
  function applyTemplate(next: string) {
    setPendingTemplate(null);
    // The whole form is replaced, so a refusal about what a field used to
    // hold describes nothing on screen any more.
    setAutoLaunchError(null);
    setScheduleError(null);

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
        queueStates: queueStates.query.isSuccess
          ? queueStates.states.map((state) => state.name)
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
    setScheduleError(null);
    patch({
      kind,
      ...(partialTouched
        ? {}
        : { partial_messages: partialMessagesDefault(kind) }),
      ...(capStranded ? { max_concurrent: String(MIN_MAX_CONCURRENT) } : {}),
    });
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
              hint="A role template fills the name, kind, served states, git tools, prompt and — on a scheduled role — its schedule; you can change anything before saving."
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
                  {pendingTemplate !== null && (
                    <ConfirmPanel
                      tone="caution"
                      message={`Start from ${pendingTemplate === BLANK_TEMPLATE ? "a blank profile" : pendingTemplate}? It overwrites the name, kind, served states, git tools, system prompt and schedule you have filled in.`}
                      confirmLabel={`Start from ${pendingTemplate === BLANK_TEMPLATE ? "blank" : pendingTemplate}`}
                      cancelLabel="Keep what I wrote"
                      onConfirm={() => {
                        applyTemplate(pendingTemplate);
                      }}
                      onCancel={() => {
                        setPendingTemplate(null);
                      }}
                    />
                  )}
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
            <>
              <Fieldset
                legend="Unattended launches"
                description="The dispatcher picks up tasks in the served states above and runs this profile on them without anyone asking. The cap holds it back only: your own launches are never refused by it."
              >
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
                {form.auto_launch && autoLaunchError === null && (
                  <UnattendedCredentialNotice unattended={unattended} />
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
              </Fieldset>

              {/* The schedule, under the same ephemeral-only rule and for the
                same reason: a scheduled run has nobody behind it. No cron
                parser ships in the bundle — whether an expression is valid,
                and when it next fires, are the server's answers (ADR 0043). */}
              <Fieldset
                legend="Schedule"
                description="A cron expression starts a session of this profile by itself, on the clock, and gives it the prompt below. Leave both empty for no schedule."
              >
                <FieldShell
                  label="Cron expression (UTC)"
                  name="profile-schedule-cron"
                  hint="Five fields — minute hour day-of-month month day-of-week — read in UTC, never in your own zone. No seconds field, no year field, no @daily."
                  error={scheduleFieldError("cron")}
                >
                  {(control) => (
                    <input
                      {...control}
                      value={form.schedule_cron}
                      onChange={(event) => {
                        patchSchedule({ schedule_cron: event.target.value });
                      }}
                      placeholder="no schedule"
                      autoComplete="off"
                      spellCheck={false}
                      disabled={save.loading}
                      className={FIELD}
                    />
                  )}
                </FieldShell>

                <ul className="text-console-muted pt-1.5 text-xs">
                  {CRON_EXAMPLES.map((example) => (
                    <li key={example.expression}>
                      <span className="text-console-text font-mono">
                        {example.expression}
                      </span>{" "}
                      — {example.meaning}
                    </li>
                  ))}
                </ul>

                <div className="pt-3">
                  <FieldShell
                    label="Schedule prompt"
                    name="profile-schedule-prompt"
                    hint="What every scheduled run is asked to do; it is the message the session opens with."
                    error={scheduleFieldError("prompt")}
                  >
                    {(control) => (
                      <textarea
                        {...control}
                        rows={4}
                        value={form.schedule_prompt}
                        onChange={(event) => {
                          patchSchedule({
                            schedule_prompt: event.target.value,
                          });
                        }}
                        spellCheck={false}
                        disabled={save.loading}
                        className={`${FIELD} resize-y`}
                      />
                    )}
                  </FieldShell>
                </div>

                {/* Before the save, from the answer the secrets notice already
                  read; after it, in the server's own words at the field. */}
                {form.schedule_cron.trim() !== "" && scheduleError === null && (
                  <UnattendedCredentialNotice unattended={unattended} />
                )}

                {/* The scheduler's own two timestamps, which only exist for a
                  stored profile. They are the server's — the next one is
                  recomputed on every read — so they describe what is saved,
                  not what is in the boxes above. */}
                {profile !== null && (
                  <div className="grid gap-3 pt-3 sm:grid-cols-2">
                    <ScheduleInstant
                      label="Next run"
                      name="profile-next-scheduled-at"
                      hint="Recomputed by the server; it follows the saved expression, not the one being typed."
                      iso={profile.next_scheduled_at}
                    />
                    <ScheduleInstant
                      label="Last run"
                      name="profile-last-scheduled-at"
                      hint="When the scheduler last decided a tick of this profile fires."
                      iso={profile.last_scheduled_at}
                    />
                  </div>
                )}
              </Fieldset>
            </>
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

          <Fieldset
            legend="Git tools"
            description="The task tracker tools are always available. These four reach the project’s git mirror."
          >
            <div className="flex flex-wrap gap-x-4 gap-y-2">
              <CheckboxList
                items={PROFILE_GATED_TOOLS.map((tool) => ({ name: tool }))}
                selected={form.mcp_tools}
                onToggle={(tool) => {
                  patch({ mcp_tools: toggleMember(form.mcp_tools, tool) });
                }}
                disabled={save.loading}
              />
            </div>
          </Fieldset>

          <ServedStatesFieldset
            projectId={projectId}
            selected={form.serves_states}
            onToggle={(name) => {
              patch({ serves_states: toggleMember(form.serves_states, name) });
            }}
            disabled={save.loading}
          />

          <SecretsFieldset
            projectId={projectId}
            backend={profile?.backend ?? PROFILE_BACKEND}
            selected={form.secrets}
            onToggle={(name) => {
              patch({ secrets: toggleMember(form.secrets, name) });
            }}
            onDeclare={(name) => {
              if (!form.secrets.includes(name)) {
                patch({ secrets: [...form.secrets, name] });
              }
            }}
            disabled={save.loading}
            error={secretNameError}
            onErrorChange={setSecretNameError}
          />
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

/**
 * One of the scheduler's timestamps: the instant in the viewer's own zone,
 * with the UTC one under it, because a schedule is written in UTC and a reader
 * checking an expression needs both without hovering anything. A profile with
 * no schedule, or an expression that can never fire, has none and shows the
 * placeholder every other missing value in the console shows.
 */
interface ScheduleInstantProps {
  label: string;
  name: string;
  hint: string;
  iso: string | null;
}

function ScheduleInstant({ label, name, hint, iso }: ScheduleInstantProps) {
  return (
    <FieldShell label={label} name={name} hint={hint}>
      {(control) => (
        <output {...control} className={`${READ_ONLY_CLASS} block`}>
          {iso === null ? (
            PLACEHOLDER
          ) : (
            <>
              {formatDateTime(iso)}
              <span className="text-console-muted block text-xs">
                {formatUtc(iso)}
              </span>
            </>
          )}
        </output>
      )}
    </FieldShell>
  );
}

/**
 * Why an unattended launch — the dispatcher's or a schedule's — would have no
 * credential to authenticate with. One notice for both, because it is one
 * rule and one answer (`SPEC.md`, "Agent profiles"; ADR 0036). An answer that
 * has not arrived, or failed, claims nothing and the save decides.
 */
function UnattendedCredentialNotice({
  unattended,
}: {
  unattended: UnattendedCredential;
}) {
  if (unattended !== "missing" && unattended !== "user_only") {
    return null;
  }

  return (
    <p className="text-state-parked pt-2 text-xs">
      {unattended === "missing"
        ? "No agent credential is stored for this project."
        : "Only your own agent credential is stored."}{" "}
      An unattended launch has no user behind it, so it needs one at the project
      or shared scope. <CredentialLink />
    </p>
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
