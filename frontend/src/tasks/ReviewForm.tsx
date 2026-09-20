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
// The hand-off can be superseded while the form is open. That 409 is the one
// refusal not shown in the server's words: the useful answer is not "the id
// does not match" but "there is newer work, read it", and the task is
// refetched so the panel above the form already shows it.

import { useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import type { Handoff, TaskDetail } from "../types";
import {
  REVIEW_ACTION,
  STATE_HINT,
  reviewCoverLine,
  reviewErrorMessage,
} from "./handoffRules";
import type { ReviewDecision } from "./handoffRules";
import { CONTROL } from "./taskChrome";
import { useTaskStore } from "./taskStore";
import { useTaskMutations } from "./useTaskMutations";

export interface ReviewFormProps {
  projectId: string;
  task: TaskDetail;
  /** The task's current hand-off; the form only ever forwards this one. */
  handoff: Handoff;
  decision: ReviewDecision;
  onDone: () => void;
}

const FIELD = `${CONTROL} w-full placeholder:text-console-muted`;

export function ReviewForm({
  projectId,
  task,
  handoff,
  decision,
  onDone,
}: ReviewFormProps) {
  const states = useTaskStore((store) => store.states);
  const { update } = useTaskMutations(projectId, task.number);

  const [state, setState] = useState("");
  const [comment, setComment] = useState("");

  const filled = state !== "" && comment.trim() !== "";

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!filled) {
      return;
    }
    update.mutate(
      {
        state,
        handoff: {
          kind: "forward",
          handoff_id: handoff.id,
          comment: comment.trim(),
          ...(decision === "none" ? {} : { review: decision }),
        },
      },
      { onSuccess: onDone },
    );
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label={REVIEW_ACTION[decision]}
      className="border-console-border bg-console-bg space-y-3 rounded border p-3"
    >
      <p className="text-console-text text-sm">
        {reviewCoverLine(decision, handoff.commit)}{" "}
        <span className="text-console-muted font-mono text-xs">
          on {handoff.source_branch}
        </span>
      </p>

      <div className="flex flex-col gap-1.5">
        <label htmlFor="review-state" className="text-console-muted text-xs">
          Move to
        </label>
        <select
          id="review-state"
          name="review-state"
          value={state}
          disabled={update.isPending}
          aria-describedby="review-state-hint"
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
        <p id="review-state-hint" className="text-console-muted text-xs">
          {STATE_HINT}
        </p>
      </div>

      <div className="flex flex-col gap-1.5">
        <label htmlFor="review-comment" className="text-console-muted text-xs">
          Comment
        </label>
        <textarea
          id="review-comment"
          name="review-comment"
          rows={4}
          value={comment}
          disabled={update.isPending}
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
      </div>

      {update.isError && (
        <Alert kind="error">{reviewErrorMessage(update.error)}</Alert>
      )}

      <div className="flex items-center gap-2">
        <SubmitButton loading={update.isPending} disabled={!filled}>
          {REVIEW_ACTION[decision]}
        </SubmitButton>
        <SubmitButton
          type="button"
          variant="ghost"
          loading={false}
          disabled={update.isPending}
          onClick={onDone}
        >
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}
