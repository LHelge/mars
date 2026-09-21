// The Dependencies section of the drawer: the edges the task has, and the
// controls that add and remove them (`SPEC.md`, "Tasks": `POST
// .../dependencies {depends_on, kind?}` and `DELETE
// .../dependencies/{dep}?kind=`).
//
// The picker searches the board snapshot with the board's own search — `#12`
// or a piece of a title (ADR 0031) — so the same query finds the same task in
// both places, and no read is made per keystroke. The task itself is never
// among the matches.
//
// A kind is always chosen and always sent back on removal, because the same
// pair of tasks may carry several edges at once (`docs/data-model.md`,
// `task_dependencies`): removing the `blocks` edge of a pair that was also
// discovered from that task leaves the provenance edge standing, and the list
// keeps the two kinds apart so what remains is visible.

import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { useMutation } from "@tanstack/react-query";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { errorMessage } from "../services/errorMessage";
import type { TaskDependencyKind, TaskDetail } from "../types";
import { DependencyList } from "./DependencyList";
import { filterTasks } from "./search";
import { CONTROL } from "../components/fieldStyles";
import { useTaskStore } from "./taskStore";
import { useAddDependency, useRemoveDependency } from "./taskWrites";

/** What each kind means from this task's side, as the select words it. */
const KIND_LABEL: Record<TaskDependencyKind, string> = {
  blocks: "blocks — must close first",
  discovered_from: "discovered from — where this came from",
  related: "related — for context only",
};

const KINDS: TaskDependencyKind[] = ["blocks", "discovered_from", "related"];

/** Long enough to choose from, short enough to keep the drawer scannable. */
const MAX_MATCHES = 8;

export interface DependencyEditorProps {
  projectId: string;
  task: TaskDetail;
}

export function DependencyEditor({ projectId, task }: DependencyEditorProps) {
  const snapshot = useTaskStore((state) => state.tasks);
  const addEdge = useAddDependency(projectId, task.number);
  // The row buttons of the list are not a form, so their write's mutation is
  // its own owner (`CLAUDE.md`, "Frontend conventions", "Submitting a form").
  const removeDependency = useMutation({
    mutationFn: useRemoveDependency(projectId, task.number),
  });

  const [query, setQuery] = useState("");
  const [target, setTarget] = useState("");
  const [kind, setKind] = useState<TaskDependencyKind>("blocks");

  const fieldId = `task-${String(task.number)}-dependency`;

  const matches = useMemo(
    () =>
      filterTasks(
        snapshot.filter((candidate) => candidate.id !== task.id),
        query,
      ).slice(0, MAX_MATCHES),
    [snapshot, query, task.id],
  );

  // A match list that no longer contains the chosen task — the query moved on
  // — must not leave a stale id in the select.
  const chosen = matches.some((match) => match.id === target) ? target : "";

  const add = useFormSubmit(async () => {
    await addEdge({ depends_on: chosen, kind });
    setQuery("");
    setTarget("");
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (chosen === "") return;
    void add.submit();
  }

  return (
    <div className="space-y-3">
      <DependencyList
        projectId={projectId}
        dependsOn={task.depends_on}
        blocks={task.blocks}
        known={task.children}
        onRemove={(dep, edgeKind) => {
          removeDependency.mutate({ depends_on: dep, kind: edgeKind });
        }}
        removing={removeDependency.isPending}
      />

      {removeDependency.isError && (
        <Alert kind="error">{errorMessage(removeDependency.error)}</Alert>
      )}

      <form
        onSubmit={onSubmit}
        aria-label="Add dependency"
        className="border-console-border bg-console-bg flex flex-col gap-2 rounded border p-2"
      >
        <div className="grid gap-2 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${fieldId}-search`} className="text-console-muted text-xs">
              Find a task
            </label>
            <input
              id={`${fieldId}-search`}
              name={`${fieldId}-search`}
              value={query}
              autoComplete="off"
              placeholder="#42 or part of a title"
              onChange={(event) => {
                setQuery(event.target.value);
              }}
              className={CONTROL}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${fieldId}-kind`} className="text-console-muted text-xs">
              Kind
            </label>
            <select
              id={`${fieldId}-kind`}
              name={`${fieldId}-kind`}
              value={kind}
              onChange={(event) => {
                setKind(event.target.value as TaskDependencyKind);
              }}
              className={CONTROL}
            >
              {KINDS.map((option) => (
                <option key={option} value={option}>
                  {KIND_LABEL[option]}
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <label htmlFor={fieldId} className="text-console-muted text-xs">
            Task
          </label>
          <select
            id={fieldId}
            name={fieldId}
            value={chosen}
            size={4}
            onChange={(event) => {
              setTarget(event.target.value);
            }}
            className={CONTROL}
          >
            {matches.length === 0 && (
              <option value="" disabled>
                No task on this board matches
              </option>
            )}
            {matches.map((match) => (
              <option key={match.id} value={match.id}>
                #{match.number} {match.title}
              </option>
            ))}
          </select>
        </div>

        {add.error !== null && <Alert kind="error">{add.error}</Alert>}

        <div className="flex justify-end">
          <SubmitButton loading={add.loading} disabled={chosen === ""}>
            Add dependency
          </SubmitButton>
        </div>
      </form>
    </div>
  );
}
