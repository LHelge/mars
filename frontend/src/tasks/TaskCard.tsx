// One card on the task board (`SPEC.md`, "Frontend", "Task board": priority,
// labels, the holding session with a link, attempts when above one, the round
// when above one against the project's `max_rounds`, assignee,
// blocked and dependency indicators, a parent badge, and the line saying the
// task waits for its author session's branch to reach the default branch).
//
// The whole card is a link to `/projects/{pid}/tasks/{number}` — the board
// with that task's drawer open — apart from the holding session, which is a
// link of its own into the transcript. A card is not draggable: a task moves
// from the drawer.
//
// Colour is information, not decoration (`CLAUDE.md`, "Frontend conventions").
// A card is grey until something about it needs attention: a critical or high
// priority, a task that is blocked, and the session currently holding it. P2
// and P3 are set in the muted tone, because "ordinary" is the default and
// should not compete for the eye.
//
// The card is memoised on task identity. The store installs a fresh snapshot
// on every refresh, so this only pays off for the renders a sibling causes —
// the search field, the drawer opening — but those are the frequent ones.

import { memo } from "react";
import { Link } from "react-router";

import { Icon, ICON_CLASS } from "../components/icons";
import type { Task } from "../types";
import { taskCardTestId } from "../utils/testIds";
import { AuthorBranchLine } from "./AuthorBranchLine";
import { taskPath } from "./taskLink";
import {
  CHIP,
  PRIORITY_COLOUR,
  PRIORITY_MEANING,
  roundLabel,
} from "./taskChrome";
import { useTaskStore } from "./taskStore";
import { selectTaskById } from "./taskStore";
import { useUsername } from "./useUsername";

export interface TaskCardProps {
  task: Task;
  /** The task named by `/projects/:id/tasks/:number`: marked where it sits. */
  selected?: boolean;
  /** The project's `max_rounds`, when the board was given it. */
  maxRounds?: number;
}

function TaskCardView({ task, selected, maxRounds }: TaskCardProps) {
  // The parent is read from the same snapshot the card came from, so no card
  // makes a request of its own to render its badge.
  const parent = useTaskStore(
    task.parent_id === null ? selectNothing : selectTaskById(task.parent_id),
  );

  const blockedBy = task.depends_on.filter(
    (dependency) => dependency.kind === "blocks",
  ).length;
  const blocking = task.blocks.length;
  const round = roundLabel(task.rounds, maxRounds);

  return (
    <article
      // The end-to-end suite addresses a card by its per-project number.
      data-testid={taskCardTestId(task.number)}
      aria-current={selected ? "true" : undefined}
      className={`bg-console-surface hover:border-console-accent/60 rounded border transition-colors ${selected ? "border-console-accent" : "border-console-border"}`}
    >
      <Link
        to={taskPath(task.project_id, task.number)}
        className="focus-visible:outline-console-accent block px-2.5 py-2 focus-visible:outline-2"
      >
        <div className="flex items-baseline gap-2">
          <span className="text-console-muted shrink-0 font-mono text-xs">
            #{task.number}
          </span>
          <span
            title={PRIORITY_MEANING[task.priority]}
            className={`shrink-0 font-mono text-xs ${PRIORITY_COLOUR[task.priority]}`}
          >
            P{task.priority}
          </span>
          {/* A card is a level under its column's heading, so an outline
              reader can tell the board's columns from the cards in them. */}
          <h4 className="text-console-text min-w-0 flex-1 text-sm">
            {task.title}
          </h4>
        </div>

        {task.needs_human_reason !== null && (
          <p className="text-state-human mt-1 flex items-center gap-1.5 text-xs">
            <Icon.needsHuman aria-hidden="true" className={ICON_CLASS} />
            <span className="min-w-0 truncate">{task.needs_human_reason}</span>
          </p>
        )}

        <div className="mt-1.5 flex flex-wrap items-center gap-1">
          {task.parent_id !== null && (
            <span
              className={`${CHIP} text-console-muted`}
              title="This task is a child of another task"
            >
              part of #{parent === undefined ? "?" : parent.number}
            </span>
          )}

          {task.blocked && (
            <span
              className={`${CHIP} border-state-failed/60 text-state-failed`}
              title="Waiting on open children or unsatisfied dependencies"
            >
              blocked
            </span>
          )}

          {(blockedBy > 0 || blocking > 0) && (
            <span
              className={`${CHIP} text-console-muted`}
              title={dependencyTitle(blockedBy, blocking)}
            >
              {blockedBy > 0 && <span>↑{blockedBy}</span>}
              {blockedBy > 0 && blocking > 0 && <span>&nbsp;</span>}
              {blocking > 0 && <span>↓{blocking}</span>}
            </span>
          )}

          {task.attempts > 1 && (
            <span
              className={`${CHIP} text-console-muted`}
              title="Sessions that have picked this task up"
            >
              {task.attempts} attempts
            </span>
          )}

          {round !== null && (
            <span
              className={`${CHIP} text-console-muted`}
              title="Revision rounds since the task last left the human state; at the limit a send-back escalates it instead"
            >
              {round}
            </span>
          )}

          <Assignee id={task.assignee_user_id} />

          {task.labels.map((label) => (
            <span key={label} className={`${CHIP} text-console-muted`}>
              {label}
            </span>
          ))}
        </div>
      </Link>

      {/* Only an unheld task can be waiting for its author's branch, so this
          and the holding session's footer never show together. */}
      <AuthorBranchLine
        task={task}
        className="border-console-border/60 border-t px-2.5 py-1.5"
      />

      {task.lease_holder_session_id !== null && (
        <div className="border-console-border/60 border-t px-2.5 py-1.5">
          <Link
            to={`/sessions/${task.lease_holder_session_id}`}
            title="The session holding this task"
            className={`${CHIP} border-console-accent/60 text-console-accent hover:bg-console-accent/10`}
          >
            held
          </Link>
        </div>
      )}
    </article>
  );
}

export const TaskCard = memo(TaskCardView);

/** No parent, no lookup: a selector that subscribes to nothing that changes. */
function selectNothing(): undefined {
  return undefined;
}

function dependencyTitle(blockedBy: number, blocking: number): string {
  const parts: string[] = [];
  if (blockedBy > 0) parts.push(`Blocked by ${String(blockedBy)}`);
  if (blocking > 0) parts.push(`Blocks ${String(blocking)}`);
  return parts.join("; ");
}

/** The assignee's username, shared with the drawer (`useUsername`). */
function Assignee({ id }: { id: string | null }) {
  const username = useUsername(id);

  if (username === null) return null;

  return (
    <span className={`${CHIP} text-console-muted`} title="Assignee">
      @{username}
    </span>
  );
}
