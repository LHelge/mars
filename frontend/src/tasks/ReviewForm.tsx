// Reviewing the task's current hand-off (`SPEC.md`, "Code hand-offs and
// review": a `PUT` whose `handoff` is `{ kind: "forward", handoff_id, comment,
// review? }`, with a different target state).
//
// One form serves all three decisions, because they differ only in what is
// recorded: approve, request changes, or pass the work on without judging it.
// What does not differ is the code under review — forwarding reuses the
// hand-off's branch and pinned commit and never picks up whatever a branch has
// done since. The form says which commit the decision covers for exactly that
// reason: an approval is about a commit, and the badge it produces will carry
// that commit forever.
//
// The hand-off can be superseded while the form is open. The form reviews the
// hand-off it opened on and no other: the id is captured at mount and the
// cover line, the branch and the submitted `handoff_id` all come from that one
// reading. Following the prop instead would quietly retarget a decision the
// reviewer has already made — the drawer rereads the task on every task event
// ("Board refresh ordering"), so an approval typed against commit A would
// leave as an approval of commit B, which nobody looked at, and the 409 that
// exists for exactly this could only fire in the moment before the refetch.
// Pinned, the request carries the superseded id and the server refuses it.
//
// A newer revision is said out loud while it is still a choice: the form shows
// that one arrived rather than swapping the commit under the comment, and the
// reviewer can cancel and read it. That 409 is the one refusal not shown in
// the server's words: the useful answer is not "the id does not match" but
// "there is newer work, read it", and the task is refetched so the panel above
// the form already shows it.

import { useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { FIELD } from "../components/fieldStyles";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import type { Handoff, TaskDetail } from "../types";
import {
  REVIEW_ACTION,
  STATE_HINT,
  reviewCoverLine,
  reviewErrorMessage,
} from "./handoffRules";
import type { ReviewDecision } from "./handoffRules";
import { useTaskStore } from "./taskStore";
import { useUpdateTask } from "./taskWrites";

export interface ReviewFormProps {
  projectId: string;
  task: TaskDetail;
  /**
   * The task's current hand-off when the form opens; the form forwards that
   * one for as long as it is open, whatever the task's current one becomes.
   */
  handoff: Handoff;
  decision: ReviewDecision;
  onDone: () => void;
}

export function ReviewForm({
  projectId,
  task,
  handoff,
  decision,
  onDone,
}: ReviewFormProps) {
  const states = useTaskStore((store) => store.states);
  const updateTask = useUpdateTask(projectId, task.number);

  // The hand-off under review, captured once. `handoff` goes on naming the
  // task's current one, which is how the form knows a revision arrived.
  const [reviewing] = useState(handoff);
  const superseded = handoff.id !== reviewing.id;

  const [state, setState] = useState("");
  const [comment, setComment] = useState("");

  const filled = state !== "" && comment.trim() !== "";

  // One owner for the decision (`CLAUDE.md`, "Frontend conventions",
  // "Submitting a form"). `mapError` is how this form says its own sentence
  // about the stale-hand-off 409; every other refusal stays the server's.
  const send = useFormSubmit(
    async () => {
      await updateTask({
        state,
        handoff: {
          kind: "forward",
          handoff_id: reviewing.id,
          comment: comment.trim(),
          ...(decision === "none" ? {} : { review: decision }),
        },
      });
      onDone();
    },
    { mapError: reviewErrorMessage },
  );

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!filled) {
      return;
    }
    void send.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label={REVIEW_ACTION[decision]}
      className="border-console-border bg-console-bg space-y-3 rounded border p-3"
    >
      <p className="text-console-text text-sm">
        {reviewCoverLine(decision, reviewing.commit)}{" "}
        <span className="text-console-muted font-mono text-xs">
          on {reviewing.source_branch}
        </span>
      </p>

      {superseded && (
        <Alert kind="warning">
          A newer revision was published while you were reviewing. This form
          still covers the commit above, so submitting it will be refused; the
          panel already shows the new revision.
        </Alert>
      )}

      <FieldShell label="Move to" name="review-state" hint={STATE_HINT}>
        {(control) => (
          <select
            {...control}
            value={state}
            disabled={send.loading}
            onChange={(event) => {
              setState(event.target.value);
            }}
            className={FIELD}
          >
            <option value="">Choose a state</option>
            {states
              .filter((row) => row.name !== task.state)
              .map((row) => (
                <option key={row.id} value={row.name}>
                  {row.name}
                </option>
              ))}
          </select>
        )}
      </FieldShell>

      <FieldShell label="Comment" name="review-comment">
        {(control) => (
          <textarea
            {...control}
            rows={4}
            value={comment}
            disabled={send.loading}
            placeholder={
              decision === "changes_requested"
                ? "What has to change, and where?"
                : "What did you check, and what happens next?"
            }
            onChange={(event) => {
              setComment(event.target.value);
            }}
            className={FIELD}
          />
        )}
      </FieldShell>

      {send.error !== null && <Alert kind="error">{send.error}</Alert>}

      <div className="flex items-center gap-2">
        <SubmitButton loading={send.loading} disabled={!filled}>
          {REVIEW_ACTION[decision]}
        </SubmitButton>
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={send.loading}
          onClick={onDone}
        >
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}
