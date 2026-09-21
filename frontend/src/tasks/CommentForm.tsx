// `POST /projects/{pid}/tasks/{id}/comments` (`SPEC.md`, "Tasks"), at the foot
// of the drawer's comment timeline.
//
// The posted comment is not written into the list. Both the open detail and
// the board behind it are invalidated instead, so what appears is an
// authoritative read — the same path the `commented` event takes, whichever
// arrives first (ADR 0022; `SPEC.md`, "Board refresh ordering").

import { useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { addComment } from "../services/tasks";
import { useSettleTask } from "./taskWrites";

// A comment is prose, not code, so this one control is deliberately not the
// monospace `CONTROL` of `components/fieldStyles`.
const COMMENT_CONTROL =
  "border-console-border bg-console-bg text-console-text rounded border px-2.5 py-1.5 text-sm disabled:opacity-50";

export interface CommentFormProps {
  projectId: string;
  /** The task's per-project number, as the drawer's route carries it. */
  number: number;
}

export function CommentForm({ projectId, number }: CommentFormProps) {
  const [body, setBody] = useState("");
  const settle = useSettleTask(projectId, number);

  const { submit, loading, error } = useFormSubmit(async () => {
    await addComment(projectId, number, body.trim());
    setBody("");
    await settle();
  });

  const empty = body.trim() === "";

  return (
    <form
      className="space-y-2"
      onSubmit={(event: FormEvent) => {
        event.preventDefault();
        void submit();
      }}
    >
      <FieldShell
        label="Add a comment"
        name="task-comment"
        hint="Markdown. Agents working this task read it with the task."
      >
        {(control) => (
          <textarea
            {...control}
            rows={3}
            value={body}
            disabled={loading}
            onChange={(event) => {
              setBody(event.target.value);
            }}
            className={COMMENT_CONTROL}
          />
        )}
      </FieldShell>

      {error !== null && <Alert kind="error">{error}</Alert>}

      <SubmitButton loading={loading} disabled={empty}>
        Comment
      </SubmitButton>
    </form>
  );
}
