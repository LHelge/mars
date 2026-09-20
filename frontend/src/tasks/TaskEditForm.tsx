// Editing a task in place (`SPEC.md`, "Tasks": `PUT
// /projects/{pid}/tasks/{id}`).
//
// The form opens on the task the drawer already read and sends only what was
// actually changed (`diffTaskInput`), so two people editing different fields
// of the same task do not overwrite each other — and a save that changed
// nothing makes no request at all.
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
import { FormField } from "../components/FormField";
import { SubmitButton } from "../components/SubmitButton";
import { useAuth } from "../hooks/useAuth";
import { projectErrorMessage } from "../pages/project/messages";
import { queryKeys } from "../services/queryKeys";
import { listUsers } from "../services/users";
import type { Task, TaskPriority } from "../types";
import { CONTROL, PRIORITIES, PRIORITY_MEANING } from "./taskChrome";
import {
  diffTaskInput,
  hasChildren,
  isEmptyUpdate,
  parentCandidates,
  taskEditValues,
} from "./taskEdit";
import { labelsError, parseLabels } from "./taskLabels";
import { useTaskStore } from "./taskStore";
import { useTaskMutations } from "./useTaskMutations";
import { useUsername } from "./useUsername";

const TITLE_MAX = 200;

/** `null` on the wire, `""` in a select: the empty option is "no id". */
const NONE = "";

export interface TaskEditFormProps {
  projectId: string;
  task: Task;
  /** Leave edit mode: the save landed, or the user cancelled. */
  onDone: () => void;
}

export function TaskEditForm({ projectId, task, onDone }: TaskEditFormProps) {
  const { user, isAdmin } = useAuth();
  const snapshot = useTaskStore((state) => state.tasks);
  const { update } = useTaskMutations(projectId, task.number);

  const original = useMemo(() => taskEditValues(task), [task]);

  const [title, setTitle] = useState(original.title);
  const [description, setDescription] = useState(original.description);
  const [priority, setPriority] = useState<TaskPriority>(original.priority);
  const [labels, setLabels] = useState(original.labels.join(" "));
  const [assignee, setAssignee] = useState(original.assignee_user_id ?? NONE);
  const [parent, setParent] = useState(original.parent_id ?? NONE);
  const [titleError, setTitleError] = useState<string | null>(null);
  const [labelError, setLabelError] = useState<string | null>(null);

  // Admin only; anyone else gets the short list below without a failed read.
  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    enabled: isAdmin,
    staleTime: 60_000,
    retry: false,
  });
  const currentAssignee = useUsername(task.assignee_user_id);

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
    if (
      task.assignee_user_id !== null &&
      !options.some((option) => option.value === task.assignee_user_id)
    ) {
      options.push({
        value: task.assignee_user_id,
        label: `@${currentAssignee ?? task.assignee_user_id}`,
      });
    }
    return options;
  }, [isAdmin, users.data, user, task.assignee_user_id, currentAssignee]);

  // A task with children cannot be given a parent at all (`SPEC.md`, "Tasks":
  // nesting is one level), so the select is disabled rather than offering a
  // list of options that would each answer 400.
  const nested = hasChildren(snapshot, task);
  const parents = useMemo(
    () => parentCandidates(snapshot, task),
    [snapshot, task],
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

    update.mutate(input, { onSuccess: onDone });
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

      <FormField
        label="Description"
        name={`${taskFieldId(task)}-description`}
        value={description}
        onChange={setDescription}
        hint="Markdown. What done looks like, and anything an agent cannot read off the repository."
      >
        <textarea
          id={`${taskFieldId(task)}-description`}
          name={`${taskFieldId(task)}-description`}
          rows={8}
          value={description}
          onChange={(event) => {
            setDescription(event.target.value);
          }}
          aria-describedby={`${taskFieldId(task)}-description-hint`}
          className={CONTROL}
        />
      </FormField>

      <div className="grid gap-3 sm:grid-cols-2">
        <FormField
          label="Priority"
          name={`${taskFieldId(task)}-priority`}
          value={String(priority)}
          onChange={(next) => {
            setPriority(Number(next) as TaskPriority);
          }}
        >
          <select
            id={`${taskFieldId(task)}-priority`}
            name={`${taskFieldId(task)}-priority`}
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
        </FormField>

        <FormField
          label="Assignee"
          name={`${taskFieldId(task)}-assignee`}
          value={assignee}
          onChange={setAssignee}
          hint={
            isAdmin
              ? "Who is accountable for the task; it does not affect which agent picks it up."
              : "You can take the task or leave it unassigned."
          }
        >
          <select
            id={`${taskFieldId(task)}-assignee`}
            name={`${taskFieldId(task)}-assignee`}
            value={assignee}
            onChange={(event) => {
              setAssignee(event.target.value);
            }}
            aria-describedby={`${taskFieldId(task)}-assignee-hint`}
            className={CONTROL}
          >
            {assignees.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </FormField>
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

      <FormField
        label="Parent task"
        name={`${taskFieldId(task)}-parent`}
        value={parent}
        onChange={setParent}
        hint={
          nested
            ? "This task has children of its own, and nesting is one level deep."
            : "The task this one is part of; it stays open until this one closes."
        }
      >
        <select
          id={`${taskFieldId(task)}-parent`}
          name={`${taskFieldId(task)}-parent`}
          value={parent}
          disabled={nested}
          onChange={(event) => {
            setParent(event.target.value);
          }}
          aria-describedby={`${taskFieldId(task)}-parent-hint`}
          className={CONTROL}
        >
          <option value={NONE}>Top-level task</option>
          {parents.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              #{candidate.number} {candidate.title}
            </option>
          ))}
        </select>
      </FormField>

      {update.isError && (
        <Alert kind="error">{projectErrorMessage(update.error)}</Alert>
      )}

      <div className="flex justify-end gap-2">
        <SubmitButton
          type="button"
          variant="ghost"
          loading={false}
          disabled={update.isPending}
          onClick={onDone}
        >
          Cancel
        </SubmitButton>
        <SubmitButton loading={update.isPending}>Save changes</SubmitButton>
      </div>
    </form>
  );
}

/**
 * Field ids are per task: the drawer can be replaced by another task's without
 * unmounting, and two forms sharing one `id` would point a label at the wrong
 * control.
 */
function taskFieldId(task: Task): string {
  return `task-${String(task.number)}`;
}
