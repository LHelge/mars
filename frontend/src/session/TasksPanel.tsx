// The `Tasks` side panel of the session view (`SPEC.md`, "Frontend", "Task
// board": the session view shows the task the session was launched for and the
// tasks it touched).
//
// One reading of the tracker, shown twice: `GET /sessions/{id}/tasks` is every
// task this session has touched, and the task it was launched for is one of
// them — a launch claims the task and a claim writes the `task_sessions` link
// (`tracker::leases::claim_for_launch`). So the launched task's row is taken
// out of that list rather than fetched again, and the rest of the list is what
// is left after it: a task is never listed twice, and a session that touched
// nothing else says so.
//
// The detail request survives as a fallback for the one case the list cannot
// answer — it failed, or the link is not there — and asks for a whole
// `TaskDetail`, comments and hand-offs included, to render a title and a
// state. That is the price of a case that should not arise, not the normal
// path.
//
// Agents move tasks while the session runs, so the panel is refreshed on every
// `session` frame — the frame that tells us anything at all changed — and on a
// slow timer for the rest. Refreshing is `invalidateQueries`, never `refetch`:
// `refetch` ignores `enabled` and would fire a request for a task this session
// was not launched for on every frame.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { Link } from "react-router";

import { Alert } from "../components/Alert";
import { EmptyState } from "../components/EmptyState";
import { LoadingState } from "../components/LoadingState";
import { queryKeys } from "../services/queryKeys";
import { listSessionTasks } from "../services/sessions";
import { getTask } from "../services/tasks";
import { taskPath } from "../tasks/taskLink";
import type { Task } from "../types";
import { getSessionStore } from "./sessionStore";
import type { SessionPanelProps } from "./sidePanels";

/** The tracker changes without us; stay roughly current when nothing streams. */
const REFETCH_MS = 60_000;

export function TasksPanel({ session }: SessionPanelProps) {
  const queryClient = useQueryClient();
  const projectId = session.project_id;
  const taskId = session.task_id;

  const touched = useQuery({
    queryKey: queryKeys.sessions.tasks(session.id),
    queryFn: () => listSessionTasks(session.id),
    refetchInterval: REFETCH_MS,
    refetchIntervalInBackground: false,
  });

  const all = touched.data ?? [];
  const fromTouched =
    taskId === null ? undefined : all.find((task) => task.id === taskId);

  const launched = useQuery({
    queryKey: queryKeys.tasks.detail(projectId, taskId ?? ""),
    queryFn: () => getTask(projectId, taskId ?? ""),
    // Only once the list has had its say and did not carry the task.
    enabled: taskId !== null && !touched.isPending && fromTouched === undefined,
  });

  // The socket's `session` frames are the panel's change notification: every
  // lease taken or released rewrites the session row too.
  useEffect(() => {
    return getSessionStore(session.id).subscribe((next, previous) => {
      if (next.session === previous.session) return;
      void queryClient.invalidateQueries({
        queryKey: queryKeys.sessions.tasks(session.id),
      });
      if (taskId !== null) {
        void queryClient.invalidateQueries({
          queryKey: queryKeys.tasks.detail(projectId, taskId),
        });
      }
    });
  }, [projectId, queryClient, session.id, taskId]);

  const launchedTask = fromTouched ?? launched.data;
  const rows = taskId === null ? all : all.filter((task) => task.id !== taskId);
  const empty = taskId === null && rows.length === 0;

  return (
    <div className="space-y-5 p-3">
      {taskId !== null && (
        <section className="space-y-2">
          <h3 className="text-console-muted font-mono text-xs">Launched for</h3>
          {launchedTask !== undefined ? (
            <TaskRow
              task={launchedTask}
              held={launchedTask.lease_holder_session_id === session.id}
            />
          ) : touched.isPending || launched.isLoading ? (
            <LoadingState label="Loading task" />
          ) : (
            <Alert kind="error">Could not load the task.</Alert>
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
      to={taskPath(task.project_id, task.number)}
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
        {held && (
          <span className="text-state-running">held by this session</span>
        )}
      </div>
    </Link>
  );
}
