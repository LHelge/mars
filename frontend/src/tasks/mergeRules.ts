// The decisions behind the drawer's merge control, kept out of the component
// so they can be read and tested on their own (`SPEC.md`, "Git": the task form
// of `MergeInput`; `ARCHITECTURE.md`, "Task tracker", "Review approval").
//
// Two of them matter enough to name:
//
// 1. Merging from the board is the approval being acted on, so the button is
//    the approval's shadow: only an approved *current* hand-off can be merged,
//    and a new revision — which arrives unreviewed — closes it again. The
//    server enforces the same rule with a 409, and the button exists to say so
//    before the request rather than after it.
// 2. What is merged is the commit the hand-off retained, not the tip of the
//    branch it came from. The form says that in a sentence, because a source
//    branch that has moved on since the approval is the normal case and the
//    commits it has gained are exactly what was not approved.

import { ApiError } from "../services/apiClient";
import { errorMessage } from "../services/errorMessage";
import type { Handoff, TaskDetail } from "../types";
import { shortSha } from "../utils/format";

/** Why the merge button is disabled, in the words it carries in its `title`. */
export const MERGE_BLOCKED = "Requires an approved current hand-off";

/** What the enabled button says on its `title`: the pinned commit, not a tip. */
export const MERGE_TITLE =
  "Merges the approved hand-off's commit, not the latest tip of its branch";

/** What the button says it does. */
export const MERGE_ACTION = "Merge approved hand-off";

/**
 * Whether this task's current hand-off may be merged. A task with no hand-off,
 * one whose current hand-off is `unreviewed` (including every fresh revision)
 * and one with `changes_requested` are all refused; an older approval in the
 * history never counts, because `task.handoff` is the only hand-off the merge
 * form can name.
 */
export function canMerge(task: Pick<TaskDetail, "handoff">): boolean {
  return task.handoff?.review_status === "approved";
}

/**
 * Whether the refusal means the task moved under the form: a hand-off that is
 * no longer current, or one whose approval was withdrawn (`SPEC.md`, "Git":
 * both are 409). The task is refetched, which re-disables the button.
 */
export function isStaleMerge(caught: unknown): boolean {
  return caught instanceof ApiError && caught.status === 409;
}

/**
 * What a failed merge says. The 409s are shown in the server's own words —
 * "handoff_id is not the task's current hand-off" and "hand-off is not
 * approved" both name the thing to go and read.
 */
export function mergeErrorMessage(caught: unknown): string {
  return errorMessage(caught);
}

/** What the form says it is about to do, naming the commit and what it omits. */
export function mergeCoverLine(
  handoff: Pick<Handoff, "commit" | "source_branch">,
): string {
  return `Merges commit ${shortSha(handoff.commit)} exactly; later commits on ${handoff.source_branch} are not included`;
}

/**
 * What a merge reports. A merge is a git action, not a task move: the task
 * stays where it is until someone moves it, which the sentence says so nobody
 * waits for a card to travel.
 */
export function mergedMessage(commit: string): string {
  return `Merged as ${shortSha(commit)} · Task state unchanged`;
}
