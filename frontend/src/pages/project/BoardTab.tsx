// The `board` tab of `/projects/:id`, and the whole of
// `/projects/:id/tasks/:number` (`SPEC.md`, "Frontend", Routes: that route is
// "the board tab with that task's drawer open", so it is this panel with the
// task named).
//
// This is the one place in the application that mounts `useTaskStream`. The
// store behind it is a singleton bound to one project at a time, so a second
// mounted stream would be a second connection writing the same snapshot; the
// panel is where the project id and the route's task number are both in hand.
//
// The route parameter arrives as it was written, not as a number: a `:number`
// that is not a task number still opens the drawer, which says so without
// asking the server. Having no `:number` at all is the other thing, and means
// no drawer.

import { TaskBoard } from "../../tasks/TaskBoard";
import { TaskDetail } from "../../tasks/TaskDetail";
import { parseTaskNumber } from "../../tasks/taskLink";
import { useTaskStream } from "../../tasks/useTaskStream";
import type { ProjectTabPanelProps } from "./tabs";

export interface BoardTabProps extends ProjectTabPanelProps {
  /** The `:number` of `/projects/:id/tasks/:number`, exactly as written. */
  taskParam?: string;
}

export function BoardTab({ project, taskParam }: BoardTabProps) {
  // The status is read from the store by the board itself; this call is here
  // for its lifetime, which is the tab's.
  useTaskStream(project.id);

  const number = taskParam === undefined ? null : parseTaskNumber(taskParam);

  return (
    <>
      <TaskBoard
        projectId={project.id}
        openTaskNumber={number ?? undefined}
        maxRounds={project.max_rounds}
      />
      {taskParam !== undefined && (
        <TaskDetail projectId={project.id} number={number} />
      )}
    </>
  );
}
