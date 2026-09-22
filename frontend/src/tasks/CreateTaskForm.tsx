// `POST /projects/{pid}/tasks` (`SPEC.md`, "Tasks"), as the board's New task
// panel.
//
// Everything the form offers comes from the snapshot already on screen: the
// state select lists this project's current states, and the parent and
// dependency pickers list its tasks. So the form can only propose what the
// board can see — and when the board is a moment behind (a state renamed, a
// task deleted) the API answers 400 or 404 and the form shows what it said,
// unchanged.
//
// A created task is not written into the store. The board refreshes through
// `invalidate()`, the same path a stream event takes (ADR 0022), so the card
// appears from an authoritative read whichever arrives first.

import { useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { SubmitButton } from "../components/SubmitButton";
import { createTask } from "../services/tasks";
import type { Task, TaskState } from "../types";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { CONTROL } from "../components/fieldStyles";
import { TaskFields } from "./TaskFields";
import { useTaskFields } from "./taskFields";
import { parseLabels } from "./taskLabels";
import { useTaskStore } from "./taskStore";

export interface CreateTaskFormProps {
  projectId: string;
  states: TaskState[];
  tasks: Task[];
  onClose: () => void;
}

export function CreateTaskForm({
  projectId,
  states,
  tasks,
  onClose,
}: CreateTaskFormProps) {
  // Where an agent would pick the work up from: the first queue state in
  // position order, which is what the API itself defaults to. A project with
  // no queue state at all falls back to its first column, so the select never
  // shows an option it is not actually set to.
  const defaultState =
    states.find((state) => state.kind === "queue")?.name ?? states[0]?.name;

  const fields = useTaskFields({
    title: "",
    description: "",
    priority: 2,
    labels: "",
    parent: "",
  });
  const [state, setState] = useState(defaultState ?? "");
  const [dependsOn, setDependsOn] = useState<string[]>([]);

  const parents = tasks.filter((task) => task.parent_id === null);

  const create = useFormSubmit(async () => {
    const { title, description, priority, labels, parent } = fields.values;
    const parsed = parseLabels(labels);
    await createTask(projectId, {
      title: title.trim(),
      ...(description.trim() === "" ? {} : { description }),
      ...(state === "" ? {} : { state }),
      priority,
      ...(parsed.labels.length === 0 ? {} : { labels: parsed.labels }),
      ...(parent === "" ? {} : { parent_id: parent }),
      ...(dependsOn.length === 0 ? {} : { depends_on: dependsOn }),
    });
    useTaskStore.getState().invalidate();
    onClose();
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!fields.validate()) return;
    void create.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="New task"
      className="border-console-border bg-console-surface flex flex-col gap-3 rounded border p-3"
    >
      <TaskFields
        idPrefix="task"
        values={fields.values}
        onChange={fields.change}
        errors={fields.errors}
        parents={parents}
        parentNoneLabel="None"
        parentHint="The task this one is part of; it stays open until this one closes."
        descriptionRows={4}
        beforePriority={
          <FieldShell label="State" name="task-state">
            {(control) => (
              <select
                {...control}
                value={state}
                onChange={(event) => {
                  setState(event.target.value);
                }}
                className={CONTROL}
              >
                {states.map((option) => (
                  <option key={option.id} value={option.name}>
                    {option.name}
                  </option>
                ))}
              </select>
            )}
          </FieldShell>
        }
        besideParent={
          <FieldShell
            label="Blocked by"
            name="task-depends-on"
            hint="Tasks that must close first. Hold Ctrl or Cmd to pick several."
          >
            {(control) => (
              <select
                {...control}
                multiple
                size={4}
                value={dependsOn}
                onChange={(event) => {
                  setDependsOn(
                    Array.from(
                      event.target.selectedOptions,
                      (option) => option.value,
                    ),
                  );
                }}
                className={CONTROL}
              >
                {tasks.map((task) => (
                  <option key={task.id} value={task.id}>
                    #{task.number} {task.title}
                  </option>
                ))}
              </select>
            )}
          </FieldShell>
        }
      />

      {create.error !== null && <Alert kind="error">{create.error}</Alert>}

      <div className="flex justify-end gap-2">
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={create.loading}
          onClick={onClose}
        >
          Cancel
        </SubmitButton>
        <SubmitButton loading={create.loading}>Create task</SubmitButton>
      </div>
    </form>
  );
}
