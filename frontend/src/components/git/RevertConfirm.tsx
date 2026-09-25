// "Revert to here" on a row of the Branches tab's history (`SPEC.md`, "Git":
// `POST /projects/{pid}/git/revert`; "Frontend", Project page and
// Confirmations; ADR 0053).
//
// A revert writes one new commit on the head whose tree is the chosen
// commit's: nothing is destroyed and the undone commits stay in the history,
// so the panel is a `caution`. What it must say before it happens is what
// will be undone — the rows above the chosen one and the tasks they brought
// in — read from the pages already loaded, which are contiguous from the head
// down, so the list is complete by construction (`history.ts`,
// `revertRange`).
//
// The panel is a form: an optional reopen of those tasks with a state and a
// required comment. One `useFormSubmit` owns the submission; the cache work —
// the history, the heads, the session branches and the board — is awaited
// inside the action, so the success note never appears before the reads that
// show the new commit. The confirmation sends `expected_head`, the head the
// table was read at, and the server's 409 `branch has moved` is not a failure
// of the user's: it is shown as advice to reload, with the reload beside it.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router";

import { useFormSubmit } from "../../hooks/useFormSubmit";
import { errorMessage } from "../../services/errorMessage";
import { isBranchMoved, revert } from "../../services/git";
import { queryKeys } from "../../services/queryKeys";
import { listTaskStates } from "../../services/taskStates";
import { taskPath } from "../../tasks/taskLink";
import type { HistoryEntry, RevertResult } from "../../types";
import { shortSha } from "../../utils/format";
import { ConfirmPanel } from "../ConfirmPanel";
import { FieldShell } from "../FieldShell";
import { FIELD } from "../fieldStyles";
import { QueryErrorAlert } from "../QueryErrorAlert";
import { rangeTasks, reopenStates } from "./history";
import { useReportBusy, type ReportBusy } from "./formState";

/** What a 409 `branch has moved` reads as: advice, with a reload beside it. */
const BRANCH_MOVED =
  "The branch has moved since this history was read, so nothing was reverted. Reload the history and choose again.";

export interface RevertConfirmProps {
  projectId: string;
  branch: string;
  /** The row the revert goes back to. */
  target: HistoryEntry;
  /** The rows above it, newest first: what the revert undoes. */
  range: HistoryEntry[];
  /** The head the table was read at. */
  expectedHead: string;
  formId: string;
  onBusy: ReportBusy;
  onCancel: () => void;
  onReverted: (result: RevertResult) => void;
}

export function RevertConfirm({
  projectId,
  branch,
  target,
  range,
  expectedHead,
  formId,
  onBusy,
  onCancel,
  onReverted,
}: RevertConfirmProps) {
  const queryClient = useQueryClient();
  const tasks = rangeTasks(range);
  const short = shortSha(target.commit);

  const [reopen, setReopen] = useState(false);
  const [chosenState, setChosenState] = useState("");
  const [comment, setComment] = useState("");
  /** The empty comment, refused before the request; the rest is `run`'s. */
  const [commentError, setCommentError] = useState<string | null>(null);
  /** No state chosen, which only a project with no queue or human state leaves. */
  const [stateError, setStateError] = useState<string | null>(null);

  // The same read, and key, as the served-states fieldset's.
  const states = useQuery({
    queryKey: queryKeys.projects.taskStates(projectId),
    queryFn: () => listTaskStates(projectId),
    enabled: tasks.length > 0,
  });
  const choices = reopenStates(states.data ?? []);
  const state = chosenState === "" ? (choices[0]?.name ?? "") : chosenState;

  const run = useFormSubmit(
    async () => {
      const result = await revert(projectId, {
        branch,
        to: target.commit,
        expected_head: expectedHead,
        reopen:
          reopen && tasks.length > 0
            ? { state, comment: comment.trim() }
            : undefined,
      });
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: queryKeys.projects.history(projectId),
        }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.projects.branches(projectId),
        }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.projects.sessionBranches(projectId),
        }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.tasks.project(projectId),
        }),
      ]);
      onReverted(result);
    },
    {
      mapError: (caught) =>
        isBranchMoved(caught) ? BRANCH_MOVED : errorMessage(caught),
    },
  );
  useReportBusy(formId, run.loading, onBusy);

  const moved = run.error === BRANCH_MOVED;
  const count = range.length;
  const commits = `${String(count)} ${count === 1 ? "commit" : "commits"}`;

  return (
    <ConfirmPanel
      tone="caution"
      message={
        <>
          This adds one commit to <span className="font-mono">{branch}</span>{" "}
          restoring it to <span className="font-mono">{short}</span>; {commits}
          {tasks.length === 0
            ? " are undone, and no task is attributed to them:"
            : " and these tasks are undone:"}
        </>
      }
      confirmLabel={`Revert ${branch} to ${short}`}
      pending={run.loading}
      error={moved ? null : run.error}
      onConfirm={() => {
        if (reopen && tasks.length > 0) {
          if (state === "") {
            setStateError("Choose the state to move them to.");
            return;
          }
          if (comment.trim() === "") {
            setCommentError("Say why these tasks are reopened.");
            return;
          }
        }
        setCommentError(null);
        setStateError(null);
        void run.submit();
      }}
      onCancel={onCancel}
    >
      <ul className="border-console-border max-h-48 space-y-0.5 overflow-y-auto border-l-2 pl-2 text-xs">
        {range.map((entry) => (
          <li key={entry.commit} className="flex flex-wrap gap-x-2">
            <span className="text-console-muted font-mono">
              {shortSha(entry.commit)}
            </span>
            <span className="text-console-text">{entry.subject}</span>
            {entry.tasks.map((task) => (
              <Link
                key={task.id}
                to={taskPath(projectId, task.number)}
                className="text-console-accent font-mono hover:underline"
              >
                #{task.number} {task.title}
              </Link>
            ))}
          </li>
        ))}
      </ul>

      {tasks.length > 0 && (
        <div className="space-y-2">
          <label className="text-console-text flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              name={`${formId}-reopen`}
              checked={reopen}
              disabled={run.loading}
              onChange={(event) => {
                setReopen(event.target.checked);
                setCommentError(null);
              }}
            />
            Reopen these tasks
          </label>

          {reopen && (
            <div className="grid gap-3 sm:grid-cols-2">
              {states.isError ? (
                <QueryErrorAlert
                  message="Could not load the project's states."
                  query={states}
                />
              ) : (
                <FieldShell
                  label="Move them to"
                  name={`${formId}-state`}
                  hint="Each of these tasks that is closed moves here and loses its current hand-off, so its next launch starts from the branch. An open one stays where it is."
                  error={stateError ?? undefined}
                >
                  {(control) => (
                    <select
                      {...control}
                      value={state}
                      disabled={run.loading || states.isPending}
                      onChange={(event) => {
                        setChosenState(event.target.value);
                        setStateError(null);
                      }}
                      className={FIELD}
                    >
                      {choices.map((choice) => (
                        <option key={choice.name} value={choice.name}>
                          {choice.name}
                        </option>
                      ))}
                    </select>
                  )}
                </FieldShell>
              )}
              <FieldShell
                label="Comment"
                name={`${formId}-comment`}
                hint="Written to each reopened task's thread as yours, beside a note naming the revert commit."
                error={commentError ?? undefined}
              >
                {(control) => (
                  <textarea
                    {...control}
                    rows={3}
                    value={comment}
                    disabled={run.loading}
                    placeholder="Why is this work being redone?"
                    onChange={(event) => {
                      setComment(event.target.value);
                      setCommentError(null);
                    }}
                    className={FIELD}
                  />
                )}
              </FieldShell>
            </div>
          )}
        </div>
      )}

      {moved && (
        <QueryErrorAlert
          kind="info"
          message={BRANCH_MOVED}
          retryLabel="Reload history"
          onRetry={() => {
            run.reset();
            void queryClient.invalidateQueries({
              queryKey: queryKeys.projects.history(projectId),
            });
          }}
        />
      )}
    </ConfirmPanel>
  );
}
