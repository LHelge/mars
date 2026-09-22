// Title, description, priority, labels and parent: the fields a task has
// whether it is being created or edited (`SPEC.md`, "Tasks").
//
// What differs between the two forms is passed in rather than branched on
// here: the create form puts its State select before Priority and its
// Blocked-by select beside Parent, the edit form puts Assignee after Priority
// and leaves Parent the width of the form. With no field beside it, Parent is
// rendered on its own instead of in a half-empty grid.

import type { ReactNode } from "react";

import { FieldShell } from "../components/FieldShell";
import { CONTROL } from "../components/fieldStyles";
import { FormField } from "../components/FormField";
import { parseTaskPriority } from "../types";
import type { Task } from "../types";
import { PRIORITIES, PRIORITY_MEANING } from "./taskChrome";
import type { TaskFieldErrors, TaskFieldValues } from "./taskFields";

export interface TaskFieldsProps {
  /**
   * Prefixes every control's `name`. Two task forms can be mounted at once —
   * one drawer tearing down as the next renders — and two controls sharing an
   * `id` point a label at the wrong one.
   */
  idPrefix: string;
  values: TaskFieldValues;
  onChange: (patch: Partial<TaskFieldValues>) => void;
  errors: TaskFieldErrors;
  /** What the parent select may offer, already filtered by the caller. */
  parents: Task[];
  /** The empty option's wording: the two forms word "no parent" differently. */
  parentNoneLabel: string;
  parentHint: string;
  /** A task with children of its own cannot be given a parent at all. */
  parentDisabled?: boolean;
  descriptionRows: number;
  /** The caller's own field, first in the priority row. */
  beforePriority?: ReactNode;
  /** The caller's own field, second in the priority row. */
  afterPriority?: ReactNode;
  /** The caller's own field beside Parent; without one Parent stands alone. */
  besideParent?: ReactNode;
}

export function TaskFields({
  idPrefix,
  values,
  onChange,
  errors,
  parents,
  parentNoneLabel,
  parentHint,
  parentDisabled = false,
  descriptionRows,
  beforePriority,
  afterPriority,
  besideParent,
}: TaskFieldsProps) {
  const parent = (
    <FieldShell
      label="Parent task"
      name={`${idPrefix}-parent`}
      hint={parentHint}
    >
      {(control) => (
        <select
          {...control}
          value={values.parent}
          disabled={parentDisabled}
          onChange={(event) => {
            onChange({ parent: event.target.value });
          }}
          className={CONTROL}
        >
          <option value="">{parentNoneLabel}</option>
          {parents.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              #{candidate.number} {candidate.title}
            </option>
          ))}
        </select>
      )}
    </FieldShell>
  );

  return (
    <>
      <FormField
        label="Title"
        name={`${idPrefix}-title`}
        value={values.title}
        onChange={(next) => {
          onChange({ title: next });
        }}
        error={errors.title ?? undefined}
        autoComplete="off"
        autoFocus
        required
      />

      <FieldShell
        label="Description"
        name={`${idPrefix}-description`}
        hint="Markdown. What done looks like, and anything an agent cannot read off the repository."
      >
        {(control) => (
          <textarea
            {...control}
            rows={descriptionRows}
            value={values.description}
            onChange={(event) => {
              onChange({ description: event.target.value });
            }}
            className={CONTROL}
          />
        )}
      </FieldShell>

      <div className="grid gap-3 sm:grid-cols-2">
        {beforePriority}

        <FieldShell label="Priority" name={`${idPrefix}-priority`}>
          {(control) => (
            <select
              {...control}
              value={String(values.priority)}
              onChange={(event) => {
                // Every option comes from the same list `parseTaskPriority`
                // reads back, so `undefined` is unreachable; it leaves the
                // priority where it is rather than inventing a P0.
                const chosen = parseTaskPriority(event.target.value);
                if (chosen !== undefined) {
                  onChange({ priority: chosen });
                }
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

        {afterPriority}
      </div>

      <FormField
        label="Labels"
        name={`${idPrefix}-labels`}
        value={values.labels}
        onChange={(next) => {
          onChange({ labels: next });
        }}
        error={errors.labels ?? undefined}
        hint="Separated by commas or spaces, for example: backend migration"
        autoComplete="off"
      />

      {besideParent === undefined ? (
        parent
      ) : (
        <div className="grid gap-3 sm:grid-cols-2">
          {parent}
          {besideParent}
        </div>
      )}
    </>
  );
}
