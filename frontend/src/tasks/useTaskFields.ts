// The half of a task form that creating and editing have in common: the draft
// the user is typing, the two checks that run before a request, and the state
// that carries them (`SPEC.md`, "Tasks": the `POST` and `PUT` bodies).
//
// It lives apart from `TaskFields.tsx` because a module that renders a
// component exports nothing else (`react-refresh/only-export-components`), and
// apart from `taskEdit.ts` because that module is the edit form's own
// arithmetic — the baseline, the diff, the parent candidates — and says
// nothing about creating a task.

import { useCallback, useState } from "react";

import type { TaskPriority } from "../types";
import { labelsError } from "./taskLabels";

/** `SPEC.md`, "Tasks": a title is 1–200 characters. */
const TITLE_MAX = 200;

/**
 * The shared fields as the form holds them while they are being typed.
 *
 * `labels` is the raw text of the input — `parseLabels` turns it into the set
 * the wire carries — and `parent` is `""` for "no parent", which is what an
 * empty `<option>` value is.
 */
export interface TaskFieldValues {
  title: string;
  description: string;
  priority: TaskPriority;
  labels: string;
  parent: string;
}

/** The two fields that can be refused before a request is made. */
export interface TaskFieldErrors {
  title: string | null;
  labels: string | null;
}

const NO_ERRORS: TaskFieldErrors = { title: null, labels: null };

/** The field checks both task forms make, in the words both of them show. */
export function validateTaskFields(values: {
  title: string;
  labels: string;
}): TaskFieldErrors {
  const trimmed = values.title.trim();
  return {
    title:
      trimmed === ""
        ? "A task needs a title"
        : trimmed.length > TITLE_MAX
          ? `A title is at most ${String(TITLE_MAX)} characters`
          : null,
    labels: labelsError(values.labels),
  };
}

/** True when `validateTaskFields` refused something. */
export function hasTaskFieldError(errors: TaskFieldErrors): boolean {
  return errors.title !== null || errors.labels !== null;
}

export interface TaskFieldsState {
  values: TaskFieldValues;
  errors: TaskFieldErrors;
  /** Applies an edit and drops the answer the edited field was refused with. */
  change: (patch: Partial<TaskFieldValues>) => void;
  /** Shows what a submission would be refused for; `true` means send it. */
  validate: () => boolean;
}

/**
 * The draft and its refusals, in one owner.
 *
 * `initial` is read once, when the form mounts: the edit form's baseline is
 * captured then and never moved (`taskEdit.ts`), and the draft has to be
 * captured with it.
 */
export function useTaskFields(initial: TaskFieldValues): TaskFieldsState {
  const [values, setValues] = useState(initial);
  const [errors, setErrors] = useState<TaskFieldErrors>(NO_ERRORS);

  const change = useCallback((patch: Partial<TaskFieldValues>) => {
    setValues((current) => ({ ...current, ...patch }));
    // An error describes the value it was raised against, so typing in that
    // field takes it away; the other field's answer stands until it is retyped
    // or the next submission replaces it.
    setErrors((current) => ({
      title: "title" in patch ? null : current.title,
      labels: "labels" in patch ? null : current.labels,
    }));
  }, []);

  function validate(): boolean {
    const next = validateTaskFields(values);
    setErrors(next);
    return !hasTaskFieldError(next);
  }

  return { values, errors, change, validate };
}
