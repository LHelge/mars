// The `board` tab of `/projects/:id`, and the whole of
// `/projects/:id/tasks/:number` (`SPEC.md`, "Frontend", Routes: that route is
// "the board tab with that task's drawer open", so it is this panel with the
// task named).
//
// This is the one place in the application that mounts `useTaskStream`. The
// store behind it is a singleton bound to one project at a time, so a second
// mounted stream would be a second connection writing the same snapshot; the
// panel is where the project id and the route's task number are both in hand.

import { useTaskStream, TaskBoard } from "../../tasks";
import type { ProjectTabPanelProps } from "./tabs";

export interface BoardTabProps extends ProjectTabPanelProps {
  /** The `:number` of `/projects/:id/tasks/:number`, when the route has one. */
  taskNumber?: number;
}

export function BoardTab({ project, taskNumber }: BoardTabProps) {
  // The status is read from the store by the board itself; this call is here
  // for its lifetime, which is the tab's.
  useTaskStream(project.id);

  return <TaskBoard projectId={project.id} openTaskNumber={taskNumber} />;
}
