// The `Tasks` side panel of the session view (`SPEC.md`, "Frontend", "Task
// board": the session view shows the task the session was launched for and the
// tasks it touched).
//
// Two readings of the same tracker: the task this session claimed at launch,
// with whether it still holds the lease, and every task it has touched since.
// Agents move tasks while the session runs, so the touched list is refetched on
// every `session` frame — the frame that tells us anything at all changed — and
// on a slow timer for the rest.

import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { Link } from "react-router";

import { Alert, EmptyState, LoadingState } from "../components";
import { queryKeys } from "../services/queryKeys";
import { listSessionTasks } from "../services/sessions";
import { getTask } from "../services/tasks";
import type { Task } from "../types";
import { getSessionStore } from "./sessionStore";
import type { SessionPanelProps } from "./sidePanels";

/** The tracker changes without us; stay roughly current when nothing streams. */
const REFETCH_MS = 60_000;

export function TasksPanel({ session }: SessionPanelProps) {
  const touched = useQuery({
    queryKey: queryKeys.sessions.tasks(session.id),
    queryFn: () => listSessionTasks(session.id),
    refetchInterval: REFETCH_MS,
    refetchIntervalInBackground: false,
  });

  const taskId = session.task_id;
  const launched = useQuery({
    queryKey: queryKeys.tasks.detail(session.project_id, taskId ?? ""),
    queryFn: () => getTask(session.project_id, taskId ?? ""),
    enabled: taskId !== null,
  });

  // The socket's `session` frames are the panel's change notification: every
  // lease taken or released rewrites the session row too.
  const refetchTouched = touched.refetch;
  const refetchLaunched = launched.refetch;
  useEffect(() => {
    return getSessionStore(session.id).subscribe((next, previous) => {
      if (next.session === previous.session) return;
      void refetchTouched();
      void refetchLaunched();
    });
  }, [session.id, refetchLaunched, refetchTouched]);

  const rows = touched.data ?? [];
  const empty = taskId === null && rows.length === 0;

  return (
    <div className="space-y-5 p-3">
      {taskId !== null && (
        <section className="space-y-2">
          <h3 className="text-console-muted font-mono text-xs">Launched for</h3>
          {launched.isPending ? (
            <LoadingState label="Loading task" />
          ) : launched.isError ? (
            <Alert kind="error">Could not load the task.</Alert>
          ) : (
            <TaskRow
              task={launched.data}
              held={launched.data.lease_holder_session_id === session.id}
            />
          )}
        </section>
      )}

      <section className="space-y-2">
        {/* A session with nothing at all says so once, not under a heading. */}
        {!empty && (
          <h3 className="text-console-muted font-mono text-xs">Touched</h3>
        )}
        {touched.isPending ? (
          <LoadingState label="Loading tasks" />
        ) : touched.isError ? (
          <Alert kind="error">Could not load the tasks of this session.</Alert>
        ) : rows.length === 0 ? (
          empty ? (
            <EmptyState
              title="No tasks"
              description="This session was not launched for a task and has not touched one."
            />
          ) : (
            <p className="text-console-muted text-sm">
              Nothing beyond the task above.
            </p>
          )
        ) : (
          <ul className="space-y-2">
            {rows.map((task) => (
              <li key={task.id}>
                <TaskRow
                  task={task}
                  held={task.lease_holder_session_id === session.id}
                />
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

interface TaskRowProps {
  task: Task;
  /** This session still holds the task's lease. */
  held: boolean;
}

function TaskRow({ task, held }: TaskRowProps) {
  return (
    <Link
      to={`/projects/${task.project_id}/tasks/${String(task.number)}`}
      className="border-console-border hover:border-console-accent block rounded border px-2 py-1.5"
    >
      <div className="flex items-baseline gap-2">
        <span className="text-console-muted font-mono text-xs">
          #{task.number}
        </span>
        <span className="text-console-text min-w-0 flex-1 text-sm">
          {task.title}
        </span>
      </div>
      <div className="text-console-muted mt-1 flex items-center gap-2 font-mono text-xs">
        <span>{task.state}</span>
        {held && <span className="text-state-running">held by this session</span>}
      </div>
    </Link>
  );
}
