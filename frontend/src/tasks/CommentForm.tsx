// `POST /projects/{pid}/tasks/{id}/comments` (`SPEC.md`, "Tasks"), at the foot
// of the drawer's comment timeline.
//
// The posted comment is not written into the list. Both the open detail and
// the board behind it are invalidated instead, so what appears is an
// authoritative read — the same path the `commented` event takes, whichever
// arrives first (ADR 0022; `SPEC.md`, "Board refresh ordering").

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { FormField } from "../components/FormField";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { addComment } from "../services/tasks";
import { taskKeys } from "./queryKeys";
import { useTaskStore } from "./taskStore";

const CONTROL =
  "border-console-border bg-console-bg text-console-text rounded border px-2.5 py-1.5 text-sm";

export interface CommentFormProps {
  projectId: string;
  /** The task's per-project number, as the drawer's route carries it. */
  number: number;
}

export function CommentForm({ projectId, number }: CommentFormProps) {
  const [body, setBody] = useState("");
  const queryClient = useQueryClient();

  const { submit, loading, error } = useFormSubmit(async () => {
    await addComment(projectId, number, body.trim());
    setBody("");
    await queryClient.invalidateQueries({
      queryKey: taskKeys.detail(projectId, number),
    });
    useTaskStore.getState().invalidate();
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
      <FormField
        label="Add a comment"
        name="task-comment"
        value={body}
        onChange={setBody}
        hint="Markdown. Agents working this task read it with the task."
      >
        <textarea
          id="task-comment"
          name="task-comment"
          rows={3}
          value={body}
          disabled={loading}
          onChange={(event) => {
            setBody(event.target.value);
          }}
          aria-describedby="task-comment-hint"
          className={`${CONTROL} disabled:opacity-50`}
        />
      </FormField>

      {error !== null && <Alert kind="error">{error}</Alert>}

      <SubmitButton loading={loading} disabled={empty}>
        Comment
      </SubmitButton>
    </form>
  );
}
