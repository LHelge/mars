// The label, hint and error around one control, and the wiring that connects
// them to it (`CLAUDE.md`, "Frontend conventions": shared UI).
//
// The control is the caller's: `children` is a function that receives the
// `id`, `aria-describedby` and `aria-invalid` the shell computed and spreads
// them onto whatever it renders. A select, a textarea and a read-only output
// therefore get the same screen-reader wiring as `FormField`'s input — where a
// hand-written `aria-describedby` used to name the hint and forget the error,
// so a refusal on a select was never announced.
//
// The control's `id` is its `name`, and the hint and error ids derive from it,
// so a field keeps the id it has always had. Two forms of the same shape can
// be on screen at once — a row's git form and the project's, two task drawers
// mid swap — and duplicate ids point a label at the wrong control, so a caller
// that can be rendered twice puts what distinguishes it into the name:
// `${formId}-target`, `task-${number}-title`, `rename-${secret.id}`.

import type { ReactNode } from "react";

/** What a control must carry for its label, hint and error to reach it. */
export interface FieldControl {
  id: string;
  name: string;
  "aria-describedby": string | undefined;
  "aria-invalid": true | undefined;
}

export interface FieldShellProps {
  label: string;
  /** The control's `name` and its `id`; unique within the document. */
  name: string;
  error?: string;
  hint?: string;
  /** Marks the label; the control still declares `required` itself. */
  required?: boolean;
  /** Renders the control with the wiring the shell computed for it. */
  children: (control: FieldControl) => ReactNode;
}

export function FieldShell({
  label,
  name,
  error,
  hint,
  required,
  children,
}: FieldShellProps) {
  const errorId = `${name}-error`;
  const hintId = `${name}-hint`;

  const control: FieldControl = {
    id: name,
    name,
    "aria-describedby":
      [error ? errorId : null, hint ? hintId : null]
        .filter(Boolean)
        .join(" ") || undefined,
    "aria-invalid": error ? true : undefined,
  };

  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={name} className="text-console-muted text-xs">
        {label}
        {required && <span className="text-console-muted"> *</span>}
      </label>

      {children(control)}

      {hint && (
        <p id={hintId} className="text-console-muted text-xs">
          {hint}
        </p>
      )}
      {error && (
        <p id={errorId} className="text-state-failed text-xs">
          {error}
        </p>
      )}
    </div>
  );
}
