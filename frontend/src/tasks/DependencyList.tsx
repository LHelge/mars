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
// Removing an edge is offered on the task's own outgoing dependencies only.
// The "Blocked by this task" group is the other task's edge — it is removed
// from that task's drawer, where the kind and the owner are both unambiguous.
// Adding edges is `DependencyEditor`, which mounts this list.

import type { ReactNode } from "react";
import { Link } from "react-router";

import { SubmitButton } from "../components/SubmitButton";
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
  /** Offers Remove on each outgoing edge; omitted, the list is read-only. */
  onRemove?: (taskId: string, kind: TaskDependencyKind) => void;
  /** A removal is in flight: every Remove waits for it. */
  removing?: boolean;
}

export function DependencyList({
  projectId,
  dependsOn,
  blocks,
  known,
  onRemove,
  removing = false,
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
                onRemove={
                  onRemove === undefined
                    ? undefined
                    : () => {
                        onRemove(edge.task_id, kind);
                      }
                }
                removing={removing}
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
              removing={removing}
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
  /** Removes this very edge — this task and this kind — when offered. */
  onRemove?: () => void;
  removing: boolean;
}

/** One edge: `#12 Rewrite the importer`, linking to that task's own drawer. */
function TaskRefLink({
  projectId,
  taskId,
  known,
  onRemove,
  removing,
}: TaskRefLinkProps) {
  const fromBoard = useTaskStore(selectTaskById(taskId));
  const task = fromBoard ?? known.find((candidate) => candidate.id === taskId);

  const remove =
    onRemove === undefined ? null : (
      <SubmitButton
        type="button"
        variant="ghost"
        loading={false}
        disabled={removing}
        onClick={onRemove}
      >
        Remove
      </SubmitButton>
    );

  if (task === undefined) {
    return (
      <li
        className="text-console-muted flex items-baseline gap-2 font-mono text-xs"
        title={`Task ${taskId}, not on this board yet`}
      >
        <span className="min-w-0 flex-1">#?</span>
        {remove}
      </li>
    );
  }

  return (
    <li className="flex min-w-0 items-baseline gap-2">
      <Link
        to={taskPath(projectId, task.number)}
        className="hover:bg-console-raised flex min-w-0 flex-1 items-baseline gap-2 rounded px-1 py-0.5"
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
      {remove}
    </li>
  );
}
