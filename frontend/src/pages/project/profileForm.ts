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
  Profile,
  ProfileInput,
  ProfileKind,
  SecretMeta,
} from "../../types";

/** `docs/data-model.md`, `agent_profiles`: the column default. */
export const DEFAULT_IDLE_TIMEOUT_SECS = 1800;

/** The shortest idle timeout the editor offers; the API's own floor is 1. */
export const MIN_IDLE_TIMEOUT_SECS = 60;

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
  };
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
