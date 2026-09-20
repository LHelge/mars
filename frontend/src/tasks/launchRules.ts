// The two decisions behind the drawer's launch buttons, kept out of the
// component so they can be read and tested on their own.
//
// Who may be launched is the API's rule, not the UI's: `SPEC.md`, "Sessions",
// answers 409 for a task that is held, blocked or in a terminal state, and for
// a project that is not `ready`. The button mirrors those four refusals so the
// user reads the reason before the request rather than after it
// (`ARCHITECTURE.md`, "Launching a session for a task": a user launch ignores
// served states, so a task in a human state is launchable — that is the
// documented way out of `needs_human`).
//
// Which profile is offered first is the board's rule: `SPEC.md`, "Frontend",
// "Task board" — the first profile of the asked-for kind that serves the
// task's current state.

import type {
  Profile,
  ProfileKind,
  ProjectStatus,
  Task,
  TaskStateKind,
} from "../types";

/**
 * The profile a launch form starts on: the first of `kind` that serves
 * `stateName`, otherwise the first of `kind`. `undefined` when the project has
 * no profile of that kind at all, which the form says instead of launching.
 */
export function defaultProfile(
  profiles: Profile[],
  kind: ProfileKind,
  stateName: string,
): Profile | undefined {
  const ofKind = profiles.filter((profile) => profile.kind === kind);
  return (
    ofKind.find((profile) => profile.serves_states.includes(stateName)) ??
    ofKind[0]
  );
}

/**
 * Why this task cannot be launched, as the sentence the button carries in its
 * `title`, or `null` when it can. The order is the order of the checks in
 * `SPEC.md`, "Sessions": the task's own condition first, the project's last.
 *
 * `stateKind` is `undefined` while the board's state list is still loading and
 * `projectStatus` while the project is; an unknown state is not assumed
 * terminal, but an unknown project is not assumed ready.
 */
export function launchDisabledReason(
  task: Pick<Task, "blocked" | "lease_holder_session_id">,
  stateKind: TaskStateKind | undefined,
  projectStatus: ProjectStatus | undefined,
): string | null {
  if (task.lease_holder_session_id !== null) {
    return "Held by a session; release it first";
  }
  if (task.blocked) {
    return "Blocked by open dependencies or children";
  }
  if (stateKind === "terminal") {
    return "Closed tasks cannot be launched";
  }
  if (projectStatus !== "ready") {
    return "Project is not ready";
  }
  return null;
}

/** How much of a hand-off commit the drawer shows; the full id is in a `title`. */
const SHORT_COMMIT = 10;

/** The leading characters of a commit id, as the launch form abbreviates one. */
export function shortCommit(commit: string): string {
  return commit.slice(0, SHORT_COMMIT);
}
