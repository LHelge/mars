// The pure part of the project settings form (`SPEC.md`, "Projects"): what the
// fields hold and what `PUT /projects/{id}` is sent.
//
// The session cap is the field with a rule worth keeping out of the component.
// It is nullable, and the endpoint distinguishes the two ways of not sending a
// number: an explicit `"max_concurrent_sessions": null` removes the cap, while
// omitting the key leaves the stored one alone. The form always means the
// former — an emptied field is "no cap" — so it always sends the key.
//
// It lives beside the component rather than in it because a module that
// renders a component exports nothing else
// (`react-refresh/only-export-components`).

import type { Project, ProjectUpdateInput } from "../../types";

/** `SPEC.md`: at least 1 when set. */
export const MIN_SESSION_CAP = 1;

export const MIN_ATTEMPTS = 1;
export const MAX_ATTEMPTS = 20;

/** `SPEC.md`, "Projects": `max_rounds` is 1–50. */
export const MIN_ROUNDS = 1;
export const MAX_ROUNDS = 50;

/** What the controls hold; the numbers are the raw text of their fields. */
export interface ProjectSettingsState {
  name: string;
  /** Empty means "still being discovered", and is then not sent. */
  default_branch: string;
  max_attempts: string;
  max_rounds: string;
  /** Empty means no cap. */
  max_concurrent_sessions: string;
  automation_paused: boolean;
}

export function toSettingsState(project: Project): ProjectSettingsState {
  return {
    name: project.name,
    default_branch: project.default_branch ?? "",
    max_attempts: String(project.max_attempts),
    max_rounds: String(project.max_rounds),
    max_concurrent_sessions:
      project.max_concurrent_sessions === null
        ? ""
        : String(project.max_concurrent_sessions),
    automation_paused: project.automation_paused,
  };
}

/** The message for an attempt limit that cannot be stored, else `null`. */
export function maxAttemptsError(raw: string): string | null {
  return boundedError(raw, MIN_ATTEMPTS, MAX_ATTEMPTS);
}

/** The message for a round limit that cannot be stored, else `null`. */
export function maxRoundsError(raw: string): string | null {
  return boundedError(raw, MIN_ROUNDS, MAX_ROUNDS);
}

/** The one rule both limits follow: a whole number in `min..=max`. */
function boundedError(raw: string, min: number, max: number): string | null {
  const parsed = Number(raw);
  if (
    raw.trim() === "" ||
    !Number.isInteger(parsed) ||
    parsed < min ||
    parsed > max
  ) {
    return `Between ${String(min)} and ${String(max)}.`;
  }
  return null;
}

/** The message for a session cap that cannot be stored, else `null`. */
export function sessionCapError(raw: string): string | null {
  if (raw.trim() === "") {
    return null;
  }
  const parsed = Number(raw);
  if (!Number.isInteger(parsed)) {
    return "A whole number, or empty for no cap.";
  }
  if (parsed < MIN_SESSION_CAP) {
    return `At least ${String(MIN_SESSION_CAP)}, or empty for no cap.`;
  }
  return null;
}

/** True while the form holds something the endpoint would refuse. */
export function settingsBlocked(state: ProjectSettingsState): boolean {
  return (
    state.name.trim() === "" ||
    maxAttemptsError(state.max_attempts) !== null ||
    maxRoundsError(state.max_rounds) !== null ||
    sessionCapError(state.max_concurrent_sessions) !== null
  );
}

/**
 * The `PUT` body. `default_branch` is left out while it is empty, because the
 * project is still discovering it and an empty name is not one; every other
 * field is always sent, the cap as an explicit `null` when the field is empty.
 */
export function toProjectUpdate(
  state: ProjectSettingsState,
): ProjectUpdateInput {
  const branch = state.default_branch.trim();
  const cap = state.max_concurrent_sessions.trim();
  return {
    name: state.name,
    ...(branch === "" ? {} : { default_branch: branch }),
    max_attempts: Number(state.max_attempts),
    max_rounds: Number(state.max_rounds),
    max_concurrent_sessions: cap === "" ? null : Number(cap),
    automation_paused: state.automation_paused,
  };
}
