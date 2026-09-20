// A task's dependency edges, grouped by kind (`SPEC.md`, "Frontend", "Task
// board": "dependencies by kind"; `docs/data-model.md`,
// `task_dependency_kind`).
//
// The API gives each edge as a task id and a kind, not as a task, so a title
// has to be resolved from something already on screen: the board snapshot
// first, then the detail's own children. A dependency on a task the board has
// not loaded — the drawer opened straight from a pasted link — shows as `#?`
// until the snapshot arrives, and fills in on the next render rather than
// making a read per edge.
//
// Adding and removing edges belongs to the actions task; this renders the
// lists.

import type { ReactNode } from "react";
import { Link } from "react-router";

import type { Task, TaskDependency, TaskDependencyKind } from "../types";
import { CHIP } from "./taskChrome";
import { taskPath } from "./taskLink";
import { selectTaskById, useTaskStore } from "./taskStore";

/** The heading each kind gets, read from this task's side of the edge. */
const KIND_HEADING: Record<TaskDependencyKind, string> = {
  blocks: "Blocks on",
  discovered_from: "Discovered from",
  related: "Related",
};

/** Kinds in the order they matter: what holds this task up comes first. */
const KIND_ORDER: TaskDependencyKind[] = ["blocks", "discovered_from", "related"];

export interface DependencyListProps {
  projectId: string;
  /** Every outgoing edge of the task, with its kind. */
  dependsOn: TaskDependency[];
  /** Ids of the tasks that have a `blocks` dependency on this one. */
  blocks: string[];
  /** Tasks the drawer already holds — its children — as a second lookup. */
  known: Task[];
}

export function DependencyList({
  projectId,
  dependsOn,
  blocks,
  known,
}: DependencyListProps) {
  if (dependsOn.length === 0 && blocks.length === 0) {
    return (
      <p className="text-console-muted text-sm">No dependencies.</p>
    );
  }

  return (
    <div className="space-y-3">
      {KIND_ORDER.map((kind) => {
        const edges = dependsOn.filter((edge) => edge.kind === kind);
        if (edges.length === 0) {
          return null;
        }
        return (
          <Group key={kind} heading={KIND_HEADING[kind]}>
            {edges.map((edge) => (
              <TaskRefLink
                key={`${kind}:${edge.task_id}`}
                projectId={projectId}
                taskId={edge.task_id}
                known={known}
              />
            ))}
          </Group>
        );
      })}

      {blocks.length > 0 && (
        <Group heading="Blocked by this task">
          {blocks.map((id) => (
            <TaskRefLink
              key={`blocks:${id}`}
              projectId={projectId}
              taskId={id}
              known={known}
            />
          ))}
        </Group>
      )}
    </div>
  );
}

function Group({
  heading,
  children,
}: {
  heading: string;
  children: ReactNode;
}) {
  return (
    <div>
      <h4 className="text-console-muted text-xs">{heading}</h4>
      <ul className="mt-1 space-y-1">{children}</ul>
    </div>
  );
}

interface TaskRefLinkProps {
  projectId: string;
  taskId: string;
  known: Task[];
}

/** One edge: `#12 Rewrite the importer`, linking to that task's own drawer. */
function TaskRefLink({ projectId, taskId, known }: TaskRefLinkProps) {
  const fromBoard = useTaskStore(selectTaskById(taskId));
  const task = fromBoard ?? known.find((candidate) => candidate.id === taskId);

  if (task === undefined) {
    return (
      <li
        className="text-console-muted font-mono text-xs"
        title={`Task ${taskId}, not on this board yet`}
      >
        #?
      </li>
    );
  }

  return (
    <li className="min-w-0">
      <Link
        to={taskPath(projectId, task.number)}
        className="hover:bg-console-raised flex items-baseline gap-2 rounded px-1 py-0.5"
      >
        <span className="text-console-muted shrink-0 font-mono text-xs">
          #{task.number}
        </span>
        <span className="text-console-text min-w-0 flex-1 truncate text-sm">
          {task.title}
        </span>
        {task.blocked && (
          <span className={`${CHIP} border-state-failed/60 text-state-failed`}>
            blocked
          </span>
        )}
      </Link>
    </li>
  );
}
