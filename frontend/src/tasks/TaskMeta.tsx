// What the task is, as a key/value grid at the top of the drawer's body.
//
// Everything here is either a fact the board also shows or a timestamp;
// nothing here is an action, which is what keeps it presentational and apart
// from `TaskDetail`. It is a dense grid in the console's monospace, the way
// the rest of the application writes ids and numbers, and a row that has
// nothing to say — no lease, no parent, no labels — is not rendered at all.
// Its one read is the project, for the `max_attempts` and `max_rounds` the
// attempt and round counts are shown against; the project page has it cached
// already.

import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Link } from "react-router";

import { Icon, ICON_CLASS } from "../components/icons";
import { projectQueries } from "../services/queryOptions";
import type { TaskDetail as TaskDetailData } from "../types";
import { formatDateTime, formatRelative, shortId } from "../utils/format";
import { CHIP, PRIORITY_COLOUR, PRIORITY_MEANING } from "./taskChrome";
import { taskPath } from "./taskLink";
import { selectTaskById, useTaskStore } from "./taskStore";
import { useUsername } from "./useUsername";

export interface TaskMetaProps {
  projectId: string;
  task: TaskDetailData;
}

export function TaskMeta({ projectId, task }: TaskMetaProps) {
  const parent = useTaskStore(
    task.parent_id === null ? selectNothing : selectTaskById(task.parent_id),
  );
  const assignee = useUsername(task.assignee_user_id);
  // The project page has already read the project, so this is the cache's
  // answer, not a request of the drawer's own.
  const project = useQuery(projectQueries.detail(projectId));
  const maxAttempts = project.data?.max_attempts;
  const maxRounds = project.data?.max_rounds;

  return (
    <div className="space-y-2">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <Row label="State">
          <span className="text-console-text font-mono text-xs">
            {task.state}
          </span>
          {task.blocked && (
            <span
              className={`${CHIP} border-state-failed/60 text-state-failed ml-2`}
              title="Waiting on open children or unsatisfied dependencies"
            >
              blocked
            </span>
          )}
        </Row>

        <Row label="Priority">
          <span
            title={PRIORITY_MEANING[task.priority]}
            className={`font-mono text-xs ${PRIORITY_COLOUR[task.priority]}`}
          >
            P{task.priority}
          </span>
        </Row>

        {task.lease_holder_session_id !== null && (
          <Row label="Held by">
            <Link
              to={`/sessions/${task.lease_holder_session_id}`}
              title={task.lease_holder_session_id}
              className="text-console-accent font-mono text-xs hover:underline"
            >
              {shortId(task.lease_holder_session_id, 8)}
            </Link>
            {task.lease_since !== null && (
              <span className="text-console-muted ml-2 text-xs">
                since{" "}
                <time
                  dateTime={task.lease_since}
                  title={formatDateTime(task.lease_since)}
                >
                  {formatRelative(task.lease_since)}
                </time>
              </span>
            )}
          </Row>
        )}

        {task.attempts > 1 && (
          <Row label="Attempts">
            <span className="text-console-text font-mono text-xs">
              {maxAttempts === undefined
                ? task.attempts
                : `${String(task.attempts)}/${String(maxAttempts)}`}
            </span>
            <span className="text-console-muted ml-2 text-xs">
              {attemptsNote(maxAttempts)}
            </span>
          </Row>
        )}

        {task.rounds > 1 && (
          <Row label="Rounds">
            <span className="text-console-text font-mono text-xs">
              {maxRounds === undefined
                ? task.rounds
                : `${String(task.rounds)}/${String(maxRounds)}`}
            </span>
            <span className="text-console-muted ml-2 text-xs">
              {roundsNote(maxRounds)}
            </span>
          </Row>
        )}

        {assignee !== null && (
          <Row label="Assignee">
            <span className="text-console-text font-mono text-xs">
              @{assignee}
            </span>
          </Row>
        )}

        {task.parent_id !== null && (
          <Row label="Parent">
            {parent === undefined ? (
              <span className="text-console-muted font-mono text-xs">
                part of #?
              </span>
            ) : (
              <Link
                to={taskPath(projectId, parent.number)}
                className="text-console-accent font-mono text-xs hover:underline"
              >
                part of #{parent.number}
              </Link>
            )}
          </Row>
        )}

        {task.labels.length > 0 && (
          <Row label="Labels">
            <span className="flex flex-wrap gap-1">
              {task.labels.map((label) => (
                <span key={label} className={`${CHIP} text-console-muted`}>
                  {label}
                </span>
              ))}
            </span>
          </Row>
        )}

        <Row label="Created">
          <Stamp iso={task.created_at} />
        </Row>
        <Row label="Updated">
          <Stamp iso={task.updated_at} />
        </Row>
        {task.closed_at !== null && (
          <Row label="Closed">
            <Stamp iso={task.closed_at} />
          </Row>
        )}
      </dl>

      {task.needs_human_reason !== null && (
        <p className="text-state-human border-state-human/50 flex max-w-prose items-start gap-1.5 border-l-2 pl-2 text-sm">
          <Icon.needsHuman
            aria-hidden="true"
            className={`mt-[0.2rem] ${ICON_CLASS}`}
          />
          <span className="min-w-0">{task.needs_human_reason}</span>
        </p>
      )}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-console-muted text-xs">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  );
}

function Stamp({ iso }: { iso: string }) {
  return (
    <time
      dateTime={iso}
      title={formatDateTime(iso)}
      className="text-console-text text-xs"
    >
      {formatRelative(iso)}
    </time>
  );
}

/**
 * What the attempt count means (`SPEC.md`, "Tasks"): claims since the task
 * last changed state, and the count at which an agent's or the reaper's
 * release sends it to the human state instead of back into its queue.
 */
function attemptsNote(maxAttempts: number | undefined): string {
  return maxAttempts === undefined
    ? "claims in this state"
    : `claims in this state; a release at ${String(maxAttempts)} escalates`;
}

/**
 * What the round count means (`ARCHITECTURE.md`, "Task tracker", "Rounds"):
 * revisions published since the task last left the human state, and the count
 * at which a send-back by an agent or a conflicting automatic merge sends it
 * to the human state instead.
 */
function roundsNote(maxRounds: number | undefined): string {
  return maxRounds === undefined
    ? "revisions since the human state"
    : `revisions since the human state; a send-back at ${String(maxRounds)} escalates`;
}

/** No parent, no lookup: a selector that subscribes to nothing that changes. */
function selectNothing(): undefined {
  return undefined;
}
