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
import { FormField } from "../components/FormField";
import { SubmitButton } from "../components/SubmitButton";
import { createTask } from "../services/tasks";
import type { Task, TaskPriority, TaskState } from "../types";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { CONTROL } from "./taskChrome";
import { labelsError, parseLabels } from "./taskLabels";
import { useTaskStore } from "./taskStore";

const TITLE_MAX = 200;

/** `SPEC.md`, "Tasks": 0 critical to 3 low, default 2. */
const PRIORITIES: { value: TaskPriority; label: string }[] = [
  { value: 0, label: "P0 — critical" },
  { value: 1, label: "P1 — high" },
  { value: 2, label: "P2 — normal" },
  { value: 3, label: "P3 — low" },
];

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

  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [state, setState] = useState(defaultState ?? "");
  const [priority, setPriority] = useState<TaskPriority>(2);
  const [labels, setLabels] = useState("");
  const [parentId, setParentId] = useState("");
  const [dependsOn, setDependsOn] = useState<string[]>([]);
  const [titleError, setTitleError] = useState<string | null>(null);
  const [labelError, setLabelError] = useState<string | null>(null);

  const parents = tasks.filter((task) => task.parent_id === null);

  const create = useFormSubmit(async () => {
    const parsed = parseLabels(labels);
    await createTask(projectId, {
      title: title.trim(),
      ...(description.trim() === "" ? {} : { description }),
      ...(state === "" ? {} : { state }),
      priority,
      ...(parsed.labels.length === 0 ? {} : { labels: parsed.labels }),
      ...(parentId === "" ? {} : { parent_id: parentId }),
      ...(dependsOn.length === 0 ? {} : { depends_on: dependsOn }),
    });
    useTaskStore.getState().invalidate();
    onClose();
  });

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const trimmed = title.trim();
    const badTitle =
      trimmed === ""
        ? "A task needs a title"
        : trimmed.length > TITLE_MAX
          ? `A title is at most ${String(TITLE_MAX)} characters`
          : null;
    const badLabels = labelsError(labels);

    setTitleError(badTitle);
    setLabelError(badLabels);
    if (badTitle !== null || badLabels !== null) return;

    void create.submit();
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label="New task"
      className="border-console-border bg-console-surface flex flex-col gap-3 rounded border p-3"
    >
      <FormField
        label="Title"
        name="task-title"
        value={title}
        onChange={(next) => {
          setTitle(next);
          setTitleError(null);
        }}
        error={titleError ?? undefined}
        autoComplete="off"
        autoFocus
        required
      />

      <FormField
        label="Description"
        name="task-description"
        value={description}
        onChange={setDescription}
        hint="Markdown. What done looks like, and anything an agent cannot read off the repository."
      >
        <textarea
          id="task-description"
          name="task-description"
          rows={4}
          value={description}
          onChange={(event) => {
            setDescription(event.target.value);
          }}
          aria-describedby="task-description-hint"
          className={CONTROL}
        />
      </FormField>

      <div className="grid gap-3 sm:grid-cols-2">
        <FormField
          label="State"
          name="task-state"
          value={state}
          onChange={setState}
        >
          <select
            id="task-state"
            name="task-state"
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
        </FormField>

        <FormField
          label="Priority"
          name="task-priority"
          value={String(priority)}
          onChange={(next) => {
            setPriority(Number(next) as TaskPriority);
          }}
        >
          <select
            id="task-priority"
            name="task-priority"
            value={String(priority)}
            onChange={(event) => {
              setPriority(Number(event.target.value) as TaskPriority);
            }}
            className={CONTROL}
          >
            {PRIORITIES.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </FormField>
      </div>

      <FormField
        label="Labels"
        name="task-labels"
        value={labels}
        onChange={(next) => {
          setLabels(next);
          setLabelError(null);
        }}
        error={labelError ?? undefined}
        hint="Separated by commas or spaces, for example: backend migration"
        autoComplete="off"
      />

      <div className="grid gap-3 sm:grid-cols-2">
        <FormField
          label="Parent task"
          name="task-parent"
          value={parentId}
          onChange={setParentId}
          hint="The task this one is part of; it stays open until this one closes."
        >
          <select
            id="task-parent"
            name="task-parent"
            value={parentId}
            onChange={(event) => {
              setParentId(event.target.value);
            }}
            aria-describedby="task-parent-hint"
            className={CONTROL}
          >
            <option value="">None</option>
            {parents.map((task) => (
              <option key={task.id} value={task.id}>
                #{task.number} {task.title}
              </option>
            ))}
          </select>
        </FormField>

        <FormField
          label="Blocked by"
          name="task-depends-on"
          value={dependsOn.join(",")}
          onChange={() => {
            // The multi-select below owns this value.
          }}
          hint="Tasks that must close first. Hold Ctrl or Cmd to pick several."
        >
          <select
            id="task-depends-on"
            name="task-depends-on"
            multiple
            size={4}
            value={dependsOn}
            onChange={(event) => {
              setDependsOn(
                Array.from(event.target.selectedOptions, (option) => option.value),
              );
            }}
            aria-describedby="task-depends-on-hint"
            className={CONTROL}
          >
            {tasks.map((task) => (
              <option key={task.id} value={task.id}>
                #{task.number} {task.title}
              </option>
            ))}
          </select>
        </FormField>
      </div>

      {create.error !== null && <Alert kind="error">{create.error}</Alert>}

      <div className="flex justify-end gap-2">
        <SubmitButton
          type="button"
          variant="ghost"
          loading={false}
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
