// The pure part of the agent-profile editor (`SPEC.md`, "Agent profiles"):
// what a new profile starts as, how a stored `Profile` becomes an editable
// body, and how the form's strings become the `ProfileInput` that is sent.
//
// `PUT` replaces the whole profile, so the editor always sends every field —
// an omitted one would silently take its default back. That is why
// `toProfileInput` copies the stored profile in full and why `toInput` never
// drops a field: the two together mean "what is on screen is what is stored".
//
// It lives beside `ProfileEditorPage.tsx` rather than in it because a module
// that renders a component exports nothing else
// (`react-refresh/only-export-components`).

import type {
  AgentCredential,
  Profile,
  ProfileInput,
  ProfileKind,
  ProfileTemplate,
  SecretMeta,
} from "../../types";

/** `docs/data-model.md`, `agent_profiles`: the column default. */
export const DEFAULT_IDLE_TIMEOUT_SECS = 1800;

/**
 * The shortest idle timeout the editor accepts, which is the API's own floor
 * (`SPEC.md`, "Agent profiles": `idle_timeout_secs` at least 1). A higher
 * editor floor would be a rule the API does not have: a profile created over
 * the API or by an agent with a 30-second timeout could not be saved from the
 * editor at all without first being changed into something else.
 */
export const MIN_IDLE_TIMEOUT_SECS = 1;

/** `SPEC.md`: `max_concurrent` defaults to 1, and 1 is also its floor. */
export const MIN_MAX_CONCURRENT = 1;

/** `SPEC.md`: `serves_states` defaults to `["ready"]`. */
export const DEFAULT_SERVES_STATES = ["ready"];

/** The only backend in v1 (`docs/data-model.md`, `agent_backend`). */
export const PROFILE_BACKEND = "claude";

/** `SPEC.md`: `permission_mode` must be `bypass`. */
export const PROFILE_PERMISSION_MODE = "bypass";

/**
 * The kind's `partial_messages` default: on for a conversation the user
 * watches token by token, off for a one-shot run nobody is reading live.
 */
export function partialMessagesDefault(kind: ProfileKind): boolean {
  return kind === "conversational";
}

/**
 * Whether the editor should treat `partial_messages` as already decided by
 * hand. The kind only picks the default while nobody has: a stored profile
 * whose flag differs from its kind's default carries a deliberate answer, and
 * switching kind to and fro must not overwrite it. A new profile (`null`) has
 * decided nothing.
 */
export function partialMessagesDecided(profile: Profile | null): boolean {
  return (
    profile !== null &&
    profile.partial_messages !== partialMessagesDefault(profile.kind)
  );
}

/** A new profile of this kind, on the image the project already uses. */
export function defaultInputForKind(
  kind: ProfileKind,
  image: string,
): ProfileInput {
  return {
    name: "",
    kind,
    backend: PROFILE_BACKEND,
    model: null,
    system_prompt: null,
    permission_mode: PROFILE_PERMISSION_MODE,
    image,
    runtime: null,
    mcp_tools: [],
    secrets: [],
    serves_states: [...DEFAULT_SERVES_STATES],
    partial_messages: partialMessagesDefault(kind),
    idle_timeout_secs: DEFAULT_IDLE_TIMEOUT_SECS,
    auto_launch: false,
    max_concurrent: MIN_MAX_CONCURRENT,
  };
}

/**
 * A stored profile as the body that would store it again: the same profile
 * without its ids, its timestamps and `is_default`, which is not part of
 * `ProfileInput` and is moved by its own action rather than by an edit.
 */
export function toProfileInput(profile: Profile): ProfileInput {
  return {
    name: profile.name,
    kind: profile.kind,
    backend: profile.backend,
    model: profile.model,
    system_prompt: profile.system_prompt,
    permission_mode: profile.permission_mode,
    image: profile.image,
    runtime: profile.runtime,
    mcp_tools: [...profile.mcp_tools],
    secrets: [...profile.secrets],
    serves_states: [...profile.serves_states],
    partial_messages: profile.partial_messages,
    idle_timeout_secs: profile.idle_timeout_secs,
    auto_launch: profile.auto_launch,
    max_concurrent: profile.max_concurrent,
  };
}

/**
 * What the controls hold. The optional strings of `ProfileInput` are plain
 * strings here — an empty one is "not set" — and the timeout is the raw text
 * of its number field, so a half-typed value is not coerced to `NaN` while it
 * is being typed.
 */
export interface ProfileFormState {
  name: string;
  kind: ProfileKind;
  model: string;
  system_prompt: string;
  image: string;
  runtime: string;
  mcp_tools: string[];
  secrets: string[];
  serves_states: string[];
  partial_messages: boolean;
  idle_timeout_secs: string;
  auto_launch: boolean;
  /** Raw text of its number field, like the timeout above. */
  max_concurrent: string;
}

export function toFormState(input: ProfileInput): ProfileFormState {
  return {
    name: input.name,
    kind: input.kind ?? "conversational",
    model: input.model ?? "",
    system_prompt: input.system_prompt ?? "",
    image: input.image ?? "",
    runtime: input.runtime ?? "",
    mcp_tools: [...(input.mcp_tools ?? [])],
    secrets: [...(input.secrets ?? [])],
    serves_states: [...(input.serves_states ?? DEFAULT_SERVES_STATES)],
    partial_messages: input.partial_messages ?? partialMessagesDefault(input.kind ?? "conversational"),
    idle_timeout_secs: String(input.idle_timeout_secs ?? DEFAULT_IDLE_TIMEOUT_SECS),
    auto_launch: input.auto_launch ?? false,
    max_concurrent: String(input.max_concurrent ?? MIN_MAX_CONCURRENT),
  };
}

/**
 * The body for `POST`/`PUT`. Blank optional text becomes `null` rather than
 * an empty string, which the API refuses; `partial_messages` is always sent,
 * so the checkbox and not the kind decides it.
 */
export function toInput(state: ProfileFormState): ProfileInput {
  const model = state.model.trim();
  const runtime = state.runtime.trim();
  return {
    name: state.name.trim(),
    kind: state.kind,
    backend: PROFILE_BACKEND,
    model: model === "" ? null : model,
    system_prompt: state.system_prompt === "" ? null : state.system_prompt,
    permission_mode: PROFILE_PERMISSION_MODE,
    image: state.image.trim(),
    runtime: runtime === "" ? null : runtime,
    mcp_tools: [...state.mcp_tools],
    secrets: [...state.secrets],
    serves_states: [...state.serves_states],
    partial_messages: state.partial_messages,
    idle_timeout_secs: Number(state.idle_timeout_secs),
    // Only an ephemeral profile may launch itself, and the controls are hidden
    // for the other kind, so the kind — not a stale checkbox — decides what is
    // sent. The flag is kept in the form state so switching back to ephemeral
    // restores what was ticked; what leaves the form is what the kind allows.
    auto_launch: state.kind === "ephemeral" && state.auto_launch,
    // Always sent, on any kind: `PUT` replaces the whole profile, and an
    // omitted cap would silently fall back to 1.
    max_concurrent: Number(state.max_concurrent),
  };
}

/** The message for a cap that cannot be stored, or `null` when it can. */
export function maxConcurrentError(raw: string): string | null {
  const parsed = Number(raw);
  if (raw.trim() === "" || !Number.isInteger(parsed)) {
    return "A whole number.";
  }
  if (parsed < MIN_MAX_CONCURRENT) {
    return `At least ${String(MIN_MAX_CONCURRENT)}.`;
  }
  return null;
}

/**
 * Whether the backend's agent credential resolves for a launch with no user
 * behind it (`SPEC.md`, "Agent profiles"; ADR 0036). The editor already reads
 * this answer for the notice beside the secrets field, so ticking auto-launch
 * can say what the server would say instead of waiting for its 400.
 *
 * `unknown` is the answer that has not arrived, failed, or is for a backend
 * this server does not report: nothing is claimed, and the save decides.
 */
export type UnattendedCredential = "resolves" | "user_only" | "missing" | "unknown";

export function unattendedCredential(
  credential: AgentCredential | null | undefined,
): UnattendedCredential {
  if (credential === undefined) {
    return "unknown";
  }
  if (credential === null) {
    return "missing";
  }
  // A `user`-scope credential belongs to whoever stored it; an unattended
  // launch has no user to resolve one for.
  return credential.scope === "user" ? "user_only" : "resolves";
}

/** The message for a timeout that cannot be stored, or `null` when it can. */
export function idleTimeoutError(raw: string): string | null {
  const parsed = Number(raw);
  if (raw.trim() === "" || !Number.isInteger(parsed)) {
    return "Whole seconds, as a number.";
  }
  if (parsed < MIN_IDLE_TIMEOUT_SECS) {
    return `At least ${String(MIN_IDLE_TIMEOUT_SECS)} seconds.`;
  }
  return null;
}

/** Adds or removes one entry of a checkbox group, keeping the rest in order. */
export function toggleMember(list: string[], value: string): string[] {
  return list.includes(value)
    ? list.filter((entry) => entry !== value)
    : [...list, value];
}

/** The value the `Start from` select carries for "no template". */
export const BLANK_TEMPLATE = "";

/**
 * `base`, or the first `base-<n>` nobody has taken. Profile names are unique
 * within a project and are what the launch form shows, so a second `reviewer`
 * becomes `reviewer-2` rather than a 409 on save.
 */
export function nextFreeName(base: string, taken: string[]): string {
  const used = new Set(taken);
  if (!used.has(base)) {
    return base;
  }
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `${base}-${String(suffix)}`;
    if (!used.has(candidate)) {
      return candidate;
    }
  }
}

/** What a template filled in, and what of it had to be left out. */
export interface TemplatePrefill {
  form: ProfileFormState;
  /**
   * Served states of the template that this project has no queue state for,
   * dropped rather than sent — they would be the 400 of `SPEC.md`, "Agent
   * profiles". The editor names them beside the select.
   */
  droppedStates: string[];
}

/** What the project contributes to a pre-fill. */
export interface TemplateContext {
  /** Prefilled as the image, like any new profile. */
  defaultImage: string;
  /** The names already taken in this project, for the suffix. */
  existingNames: string[];
  /**
   * The names of this project's `queue` states, or `null` while they are
   * unknown — the list has not arrived, or reading it failed. Unknown keeps
   * the template's states, which the editor already flags one by one.
   */
  queueStates: string[] | null;
}

/**
 * A role template as the form that would create it (`SPEC.md`, "Role profile
 * templates"). Everything the template does not carry is the ordinary default
 * of a new profile, and `is_default` is never applied: the flag is moved by
 * its own action, not by creating a profile.
 */
export function prefillFromTemplate(
  template: ProfileTemplate,
  context: TemplateContext,
): TemplatePrefill {
  const known = context.queueStates;
  const kept =
    known === null
      ? [...template.serves_states]
      : template.serves_states.filter((state) => known.includes(state));
  const droppedStates =
    known === null
      ? []
      : template.serves_states.filter((state) => !known.includes(state));

  return {
    form: toFormState({
      ...defaultInputForKind(template.kind, context.defaultImage),
      name: nextFreeName(template.name, context.existingNames),
      system_prompt: template.system_prompt,
      mcp_tools: [...template.mcp_tools],
      serves_states: kept,
    }),
    droppedStates,
  };
}

/** One offer of the editor's secrets picker. */
export interface SecretOption {
  name: string;
  orchestrator_only: boolean;
}

/**
 * The scopes a session resolves over, as one sorted list of names. A name
 * defined in more than one scope appears once; the later scope wins, which is
 * the order sessions resolve them in (`docs/data-model.md`, `secret_scope`).
 *
 * An agent credential is not offered: it is injected into every session of its
 * backend without being declared, and a profile that lists one is a 400
 * (`SPEC.md`, "Agent profiles"; ADR 0036). `credential_for` is the server's own
 * answer to "is this name a credential", so the picker needs no name table.
 */
export function mergeSecretOptions(
  lists: (SecretMeta[] | undefined)[],
): SecretOption[] {
  const byName = new Map<string, SecretOption>();
  for (const list of lists) {
    for (const meta of list ?? []) {
      if (meta.credential_for !== null) {
        continue;
      }
      byName.set(meta.name, {
        name: meta.name,
        orchestrator_only: meta.orchestrator_only,
      });
    }
  }
  return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
}
