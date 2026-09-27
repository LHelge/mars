// Every session that has touched the task, with the one holding it now marked
// (`SPEC.md`, "Frontend", "Task board": links into their transcripts).
//
// The list is a reading of the task, not of the session list, so it says only
// what the task itself records: an id to follow, and when that session first
// and last touched the work.

import { Link } from "react-router";

import type { TaskDetail as TaskDetailData } from "../types";
import { formatDateTime, formatRelative, shortId } from "../utils/format";
import { CHIP } from "./taskChrome";
import { TAP_INLINE } from "../components/fieldStyles";

export interface TaskSessionsProps {
  task: TaskDetailData;
}

export function TaskSessions({ task }: TaskSessionsProps) {
  if (task.sessions.length === 0) {
    return (
      <p className="text-console-muted text-sm">
        No session has touched this task.
      </p>
    );
  }

  return (
    <ul className="space-y-1">
      {task.sessions.map((touch) => {
        const holding = touch.session_id === task.lease_holder_session_id;
        return (
          <li
            key={touch.session_id}
            className={`flex flex-wrap items-baseline gap-x-3 gap-y-1 rounded px-2 py-1 ${holding ? "border-console-accent/60 bg-console-accent/5 border" : ""}`}
          >
            <Link
              to={`/sessions/${touch.session_id}`}
              title={touch.session_id}
              className={`text-console-accent font-mono text-xs hover:underline ${TAP_INLINE}`}
            >
              {shortId(touch.session_id, 8)}
            </Link>
            {holding && (
              <span
                className={`${CHIP} border-console-accent/60 text-console-accent`}
              >
                holding
              </span>
            )}
            <span className="text-console-muted text-xs">
              first{" "}
              <time
                dateTime={touch.first_touched_at}
                title={formatDateTime(touch.first_touched_at)}
              >
                {formatRelative(touch.first_touched_at)}
              </time>
            </span>
            <span className="text-console-muted text-xs">
              last{" "}
              <time
                dateTime={touch.last_touched_at}
                title={formatDateTime(touch.last_touched_at)}
              >
                {formatRelative(touch.last_touched_at)}
              </time>
            </span>
          </li>
        );
      })}
    </ul>
  );
}
