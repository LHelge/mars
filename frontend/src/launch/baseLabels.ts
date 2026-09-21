// What a launch form says about the commit a session will start from
// (`SPEC.md`, "Sessions": with no `base_ref` and a task that has a hand-off,
// selection and claim happen atomically, so the server picks the hand-off
// commit; `SPEC.md`, "Frontend", "Hand-off controls": both launch controls
// default to the hand-off commit and visibly disclose any base override).
//
// The wording is here rather than in either form because both forms say it,
// and a base the user reads one way on the project page and another way in the
// drawer is two claims about one launch.

import { shortSha } from "../utils/format";
import type { Handoff } from "../types";

/**
 * What the empty entry of `BaseRefSelect` is called — the thing the server
 * would pick with no `base_ref`: the task's hand-off commit when it has one,
 * otherwise the project's default branch.
 */
export function baseRefDefaultLabel(
  handoff: Handoff | null,
  defaultBranch: string | null,
): string {
  if (handoff !== null) {
    return `Hand-off commit ${shortSha(handoff.commit)}`;
  }
  return defaultBranch === null
    ? "Project default"
    : `Project default (${defaultBranch})`;
}
