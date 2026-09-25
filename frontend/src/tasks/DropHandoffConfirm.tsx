// Dropping a task's current hand-off from the drawer (`SPEC.md`, "Code
// hand-offs and review": `POST .../drop-handoff` with a required `comment`;
// "Frontend", "Hand-off controls").
//
// A drop is what a person does when the pinned commit is known to be bad —
// built on the wrong base, merged and rolled back — and the next agent must
// not start from it. It clears the pointer and nothing else, so the panel says
// both halves of that before it happens: the next launch starts from the
// default branch, and the record stays in the history below. That makes it a
// `caution` rather than a `danger`: nothing is destroyed.
//
// The comment is required because the drop writes it to the task's thread as
// the user's, and the next agent reads "why" there. It is checked here before
// the request, in the server's own terms, and the server's refusals — a 409
// when someone else dropped or replaced the hand-off first — come back through
// the one owner of the submission.

import { useState } from "react";

import { ConfirmPanel } from "../components/ConfirmPanel";
import { FieldShell } from "../components/FieldShell";
import { FIELD } from "../components/fieldStyles";
import { useFormSubmit } from "../hooks/useFormSubmit";
import type { TaskDetail } from "../types";
import { shortSha } from "../utils/format";
import { useDrawerEscape } from "./drawerEscape";
import { useDropHandoff } from "./taskWrites";

export interface DropHandoffConfirmProps {
  projectId: string;
  task: TaskDetail;
  /** The panel closes the confirmation, on `Cancel` and after the drop. */
  onDone: () => void;
}

export function DropHandoffConfirm({
  projectId,
  task,
  onDone,
}: DropHandoffConfirmProps) {
  const dropHandoff = useDropHandoff(projectId, task.number);
  const [comment, setComment] = useState("");
  /** The empty comment, refused before the request; the rest is `drop`'s. */
  const [commentError, setCommentError] = useState<string | null>(null);

  // One owner for the drop (`CLAUDE.md`, "Frontend conventions", "Submitting
  // a form"): the write settles the drawer and the board before it closes.
  const drop = useFormSubmit(async () => {
    await dropHandoff(comment.trim());
    onDone();
  });

  // Escape withdraws the confirmation as `Cancel` does, and is shut while the
  // drop is in flight for the same reason `Cancel` is (`drawerEscape.ts`).
  useDrawerEscape(onDone, !drop.loading);

  const number = `#${String(task.number)}`;
  const commit = task.handoff === null ? null : shortSha(task.handoff.commit);

  return (
    <ConfirmPanel
      tone="caution"
      message={
        <>
          Drop the current hand-off of {number}
          {commit === null ? "" : ` (${commit})`}. The next launch for this task
          starts from the default branch. The hand-off stays in the task&rsquo;s
          history, and the task keeps its state and holder.
        </>
      }
      confirmLabel={`Drop hand-off of ${number}`}
      pending={drop.loading}
      error={drop.error}
      onConfirm={() => {
        if (comment.trim() === "") {
          setCommentError("Say why the hand-off is dropped.");
          return;
        }
        setCommentError(null);
        void drop.submit();
      }}
      onCancel={onDone}
    >
      <FieldShell
        label="Comment"
        name="drop-handoff-comment"
        hint="Written to the task's thread as yours, for the next agent to read."
        error={commentError ?? undefined}
      >
        {(control) => (
          <textarea
            {...control}
            rows={3}
            value={comment}
            disabled={drop.loading}
            placeholder="Why must the next agent not start from this commit?"
            onChange={(event) => {
              setComment(event.target.value);
              setCommentError(null);
            }}
            className={FIELD}
          />
        )}
      </FieldShell>
    </ConfirmPanel>
  );
}
