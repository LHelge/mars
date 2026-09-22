// Publishing a new revision from the drawer (`SPEC.md`, "Code hand-offs and
// review": a `PUT` whose `handoff` is `{ kind: "revision", source_session_id,
// commit, comment }`, together with a different target state).
//
// The form is four fields in the order the decision is made: which session's
// work this is, which commit of it, what the next agent needs to know, and
// where the task goes. Nothing is prefilled except a single unambiguous
// source, because every other default would be the UI choosing whose branch
// gets published — the one thing `SPEC.md`, "Frontend", "Hand-off controls",
// says it must never do silently.
//
// The commit is checked here only so the obvious mistake (a branch name, an
// abbreviation) is refused before a round trip, in the server's own words. The
// real check is the server's: the source session's synced tip must equal this
// exact commit, and a 409 saying otherwise is shown verbatim, because it names
// both tips and is the most useful sentence on the screen.

import { useState } from "react";
import type { FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { FIELD } from "../components/fieldStyles";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { projectQueries } from "../services/queryOptions";
import type { TaskDetail } from "../types";
import { shortId } from "../utils/format";
import {
  COMMIT_HINT,
  commitIdError,
  defaultSourceSession,
  orderSessionsForPicker,
} from "./handoffRules";
import { useDrawerEscape } from "./drawerEscape";
import { TargetStateSelect } from "./TargetStateSelect";
import { useUpdateTask } from "./taskWrites";

export interface RevisionFormProps {
  projectId: string;
  task: TaskDetail;
  /** The panel closes the form again once the revision is published. */
  onDone: () => void;
}

export function RevisionForm({ projectId, task, onDone }: RevisionFormProps) {
  const updateTask = useUpdateTask(projectId, task.number);

  const sessions = useQuery(projectQueries.sessions(projectId));

  const rows = orderSessionsForPicker(
    sessions.data ?? [],
    task.sessions.map((touch) => touch.session_id),
  );

  const [source, setSource] = useState<string | null>(null);
  const [commit, setCommit] = useState("");
  const [comment, setComment] = useState("");
  const [state, setState] = useState("");
  /** The commit's shape, checked here; the server's refusals are `publish`'s. */
  const [commitError, setCommitError] = useState<string | null>(null);

  // Derived until the user touches it, so the single-candidate default is
  // right the moment the session list lands and no effect writes state.
  const chosen = source ?? defaultSourceSession(rows);

  const commitProblem = commitIdError(commit);
  // The commit's *shape* is not part of readiness: a button that goes dead
  // while someone types a commit id says nothing about why. It is checked on
  // submit instead, and answered in a sentence.
  const filled =
    chosen !== "" &&
    commit.trim() !== "" &&
    comment.trim() !== "" &&
    state !== "";

  // One owner for the publish (`CLAUDE.md`, "Frontend conventions",
  // "Submitting a form"); the commit check above stays the form's own.
  const publish = useFormSubmit(async () => {
    await updateTask({
      state,
      handoff: {
        kind: "revision",
        source_session_id: chosen,
        commit: commit.trim(),
        comment: comment.trim(),
      },
    });
    onDone();
  });

  // Escape leaves the form as `Cancel` does, and is shut while the revision is
  // being published for the same reason it is (`drawerEscape.ts`).
  useDrawerEscape(onDone, !publish.loading);

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!filled) {
      return;
    }
    if (commitProblem !== null) {
      setCommitError(commitProblem);
      return;
    }
    setCommitError(null);
    void publish.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="Publish revision"
      className="border-console-border bg-console-bg space-y-3 rounded border p-3"
    >
      {sessions.isError && (
        <Alert kind="error">Could not load the project&rsquo;s sessions.</Alert>
      )}

      <FieldShell label="Source session" name="handoff-source">
        {(control) => (
          <select
            {...control}
            value={chosen}
            disabled={publish.loading}
            onChange={(event) => {
              setSource(event.target.value);
            }}
            className={FIELD}
          >
            <option value="">Choose the session whose work this is</option>
            {rows.map((session) => (
              <option
                key={session.id}
                value={session.id}
                disabled={session.branch === null}
              >
                {session.title ?? "Untitled session"} — {session.state} —{" "}
                {shortId(session.id, 8)}
                {session.branch === null ? " (no branch yet)" : ""}
              </option>
            ))}
          </select>
        )}
      </FieldShell>

      <FieldShell
        label="Commit"
        name="handoff-commit"
        hint={COMMIT_HINT}
        error={commitError ?? undefined}
      >
        {(control) => (
          <input
            {...control}
            value={commit}
            spellCheck={false}
            autoComplete="off"
            disabled={publish.loading}
            placeholder="0000000000000000000000000000000000000000"
            onChange={(event) => {
              setCommit(event.target.value);
              setCommitError(null);
            }}
            className={FIELD}
          />
        )}
      </FieldShell>

      <FieldShell label="Comment" name="handoff-comment">
        {(control) => (
          <textarea
            {...control}
            rows={4}
            value={comment}
            disabled={publish.loading}
            placeholder="What is in this commit, and what should the next agent do with it?"
            onChange={(event) => {
              setComment(event.target.value);
            }}
            className={FIELD}
          />
        )}
      </FieldShell>

      <TargetStateSelect
        id="handoff-state"
        task={task}
        value={state}
        onChange={setState}
        disabled={publish.loading}
      />

      {publish.error !== null && <Alert kind="error">{publish.error}</Alert>}

      <div className="flex items-center gap-2">
        <SubmitButton loading={publish.loading} disabled={!filled}>
          Publish revision
        </SubmitButton>
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={publish.loading}
          onClick={onDone}
        >
          Cancel
        </SubmitButton>
      </div>
    </form>
  );
}
