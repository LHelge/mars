// Editing a task in place (`SPEC.md`, "Tasks": `PUT
// /projects/{pid}/tasks/{id}`).
//
// The form opens on the task the drawer already read and sends only what was
// actually changed (`diffTaskInput`), so two people editing different fields
// of the same task do not overwrite each other — and a save that changed
// nothing makes no request at all.
//
// That promise rests on the baseline standing still: the draft fields are
// captured when the form opens, so the values they are compared against are
// captured then too and never again. A refresh while the form is open — an
// SSE event, a board reread (`SPEC.md`, "Frontend", "Board refresh ordering")
// — would otherwise move the baseline under the untouched fields and turn a
// title edit into a write of whatever the server changed meanwhile. The form
// keeps editing the task it opened on and says so when the server's copy has
// moved away from it.
//
// What the selects can offer comes from what is already on screen: the parent
// candidates from the board snapshot, the assignee from the user list an
// administrator may read. A non-administrator cannot list users (`GET /users`
// is admin-only), so they get the two assignments they can make without one:
// themselves, and nobody. Whoever holds the task now is always among the
// options, named, so opening the form never silently reassigns it.
//
// Refusals are the API's own words. A nesting violation, an unknown assignee
// and a task deleted mid-edit all arrive as `{status, error}` and are shown
// unchanged; the 404 also refetches the task, which turns the drawer into its
// not-found state.

import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { FormEvent } from "react";

import { Alert } from "../components/Alert";
import { FieldShell } from "../components/FieldShell";
import { CONTROL } from "../components/fieldStyles";
import { FormField } from "../components/FormField";
import { SubmitButton } from "../components/SubmitButton";
import { useAuth } from "../hooks/useAuth";
import { useFormSubmit } from "../hooks/useFormSubmit";
import { queryKeys } from "../services/queryKeys";
import { listUsers } from "../services/users";
import type { TaskDetail, TaskPriority, UpdateTaskInput } from "../types";
import { PRIORITIES, PRIORITY_MEANING } from "./taskChrome";
import {
  diffTaskInput,
  isEmptyUpdate,
  parentCandidates,
  taskEditValues,
  taskEditValuesDiffer,
} from "./taskEdit";
import { labelsError, parseLabels } from "./taskLabels";
import { useTaskStore } from "./taskStore";
import { useUpdateTask } from "./taskWrites";
import { useUsername } from "./useUsername";

const TITLE_MAX = 200;

/** `null` on the wire, `""` in a select: the empty option is "no id". */
const NONE = "";

export interface TaskEditFormProps {
  projectId: string;
  /** The drawer's own read of the task: `children` says whether it nests. */
  task: TaskDetail;
  /** Leave edit mode: the save landed, or the user cancelled. */
  onDone: () => void;
}

export function TaskEditForm({ projectId, task, onDone }: TaskEditFormProps) {
  const { user, isAdmin } = useAuth();
  const snapshot = useTaskStore((state) => state.tasks);
  const updateTask = useUpdateTask(projectId, task.number);

  // Captured once, with the drafts below: the baseline of a diff has to be the
  // reading the drafts were taken from, not whatever the prop holds by the
  // time the user presses save.
  const [original] = useState(() => taskEditValues(task));

  const [title, setTitle] = useState(original.title);
  const [description, setDescription] = useState(original.description);
  const [priority, setPriority] = useState<TaskPriority>(original.priority);
  const [labels, setLabels] = useState(original.labels.join(" "));
  const [assignee, setAssignee] = useState(original.assignee_user_id ?? NONE);
  const [parent, setParent] = useState(original.parent_id ?? NONE);
  const [titleError, setTitleError] = useState<string | null>(null);
  const [labelError, setLabelError] = useState<string | null>(null);

  // One owner for the save: the in-flight guard, the pending flag and the
  // refusal, in the API's own words (`CLAUDE.md`, "Frontend conventions",
  // "Submitting a form"). The field checks above stay the form's own.
  const save = useFormSubmit(async (input: UpdateTaskInput) => {
    await updateTask(input);
    onDone();
  });

  // Admin only; anyone else gets the short list below without a failed read.
  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    enabled: isAdmin,
    staleTime: 60_000,
  });
  const currentAssignee = useUsername(task.assignee_user_id);
  // The draft's own holder, which a reassignment on the server can take out of
  // the list below: a select whose value is not among its options shows the
  // wrong name for what a save would leave alone.
  const draftAssignee = useUsername(assignee === NONE ? null : assignee);

  const assignees = useMemo(() => {
    const options: { value: string; label: string }[] = [
      { value: NONE, label: "Unassigned" },
    ];
    if (isAdmin && users.data !== undefined) {
      for (const candidate of users.data) {
        options.push({ value: candidate.id, label: `@${candidate.username}` });
      }
    } else if (user !== null) {
      options.push({ value: user.id, label: `@${user.username} (you)` });
    }
    for (const [id, name] of [
      [task.assignee_user_id, currentAssignee],
      [assignee === NONE ? null : assignee, draftAssignee],
    ] as const) {
      if (id !== null && !options.some((option) => option.value === id)) {
        options.push({ value: id, label: `@${name ?? id}` });
      }
    }
    return options;
  }, [
    isAdmin,
    users.data,
    user,
    task.assignee_user_id,
    currentAssignee,
    assignee,
    draftAssignee,
  ]);

  // A task with children cannot be given a parent at all (`SPEC.md`, "Tasks":
  // nesting is one level), so the select is disabled rather than offering a
  // list of options that would each answer 400.
  const nested = task.children.length > 0;
  const parents = useMemo(
    () => parentCandidates(snapshot, task),
    [snapshot, task],
  );

  // The save is still safe — it carries only the fields this form changed —
  // but the values on screen are no longer the ones the task has.
  const moved = useMemo(
    () => taskEditValuesDiffer(original, taskEditValues(task)),
    [original, task],
  );

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

    const input = diffTaskInput(original, {
      title,
      description,
      priority,
      labels: parseLabels(labels).labels,
      assignee_user_id: assignee === NONE ? null : assignee,
      parent_id: parent === NONE ? null : parent,
    });

    if (isEmptyUpdate(input)) {
      onDone();
      return;
    }

    void save.submit(input);
  }

  return (
    <form
      onSubmit={onSubmit}
      aria-label={`Edit task #${String(task.number)}`}
      className="border-console-border bg-console-bg flex flex-col gap-3 rounded border p-3"
    >
      <FormField
        label="Title"
        name={`${taskFieldId(task)}-title`}
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

      <FieldShell
        label="Description"
        name={`${taskFieldId(task)}-description`}
        hint="Markdown. What done looks like, and anything an agent cannot read off the repository."
      >
        {(control) => (
          <textarea
            {...control}
            rows={8}
            value={description}
            onChange={(event) => {
              setDescription(event.target.value);
            }}
            className={CONTROL}
          />
        )}
      </FieldShell>

      <div className="grid gap-3 sm:grid-cols-2">
        <FieldShell label="Priority" name={`${taskFieldId(task)}-priority`}>
          {(control) => (
            <select
              {...control}
              value={String(priority)}
              onChange={(event) => {
                setPriority(Number(event.target.value) as TaskPriority);
              }}
              className={CONTROL}
            >
              {PRIORITIES.map((value) => (
                <option key={value} value={value}>
                  {PRIORITY_MEANING[value]}
                </option>
              ))}
            </select>
          )}
        </FieldShell>

        <FieldShell
          label="Assignee"
          name={`${taskFieldId(task)}-assignee`}
          hint={
            isAdmin
              ? "Who is accountable for the task; it does not affect which agent picks it up."
              : "You can take the task or leave it unassigned."
          }
        >
          {(control) => (
            <select
              {...control}
              value={assignee}
              onChange={(event) => {
                setAssignee(event.target.value);
              }}
              className={CONTROL}
            >
              {assignees.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          )}
        </FieldShell>
      </div>

      <FormField
        label="Labels"
        name={`${taskFieldId(task)}-labels`}
        value={labels}
        onChange={(next) => {
          setLabels(next);
          setLabelError(null);
        }}
        error={labelError ?? undefined}
        hint="Separated by commas or spaces, for example: backend migration"
        autoComplete="off"
      />

      <FieldShell
        label="Parent task"
        name={`${taskFieldId(task)}-parent`}
        hint={
          nested
            ? "This task has children of its own, and nesting is one level deep."
            : "The task this one is part of; it stays open until this one closes."
        }
      >
        {(control) => (
          <select
            {...control}
            value={parent}
            disabled={nested}
            onChange={(event) => {
              setParent(event.target.value);
            }}
            className={CONTROL}
          >
            <option value={NONE}>Top-level task</option>
            {parents.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                #{candidate.number} {candidate.title}
              </option>
            ))}
          </select>
        )}
      </FieldShell>

      {moved && (
        <Alert kind="warning">
          This task changed while you were editing. Saving sends only the fields
          you changed here; the rest keep their new values.
        </Alert>
      )}

      {save.error !== null && <Alert kind="error">{save.error}</Alert>}

      <div className="flex justify-end gap-2">
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={save.loading}
          onClick={onDone}
        >
          Cancel
        </SubmitButton>
        <SubmitButton loading={save.loading}>Save changes</SubmitButton>
      </div>
    </form>
  );
}

/**
 * Field ids are per task. The drawer keys this form's subtree by `task.id`
 * (`TaskDetail`), so another task's form is a different mount; the per-task id
 * keeps that true in the DOM as well, where two forms sharing one `id` — one
 * being torn down as the next renders — would point a label at the wrong
 * control.
 */
function taskFieldId(task: TaskDetail): string {
  return `task-${String(task.number)}`;
}
