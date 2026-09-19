// Labelled field with its hint and error, wired for screen readers
// (`CLAUDE.md`, "Frontend conventions": shared UI).

import type { ReactNode } from "react";

export interface FormFieldProps {
  label: string;
  /** Also the control's `id`, so the label points at it. */
  name: string;
  type?: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
  hint?: string;
  /**
   * Passed through verbatim. A password field always says which password it is
   * — `current-password` or `new-password` — and never falls back to `on`.
   */
  autoComplete?: string;
  required?: boolean;
  autoFocus?: boolean;
  disabled?: boolean;
  /**
   * A custom control (select, textarea, ...) rendered in place of the input.
   * It carries `id={name}` itself and applies `value`/`onChange`.
   */
  children?: ReactNode;
}

export function FormField({
  label,
  name,
  type = "text",
  value,
  onChange,
  error,
  hint,
  autoComplete,
  required,
  autoFocus,
  disabled,
  children,
}: FormFieldProps) {
  const errorId = `${name}-error`;
  const hintId = `${name}-hint`;
  const describedBy =
    [error ? errorId : null, hint ? hintId : null].filter(Boolean).join(" ") ||
    undefined;

  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={name} className="text-console-muted text-xs">
        {label}
        {required && <span className="text-console-muted"> *</span>}
      </label>

      {children ?? (
        <input
          id={name}
          name={name}
          type={type}
          value={value}
          onChange={(event) => {
            onChange(event.target.value);
          }}
          autoComplete={autoComplete}
          required={required}
          // Login and invite pages put the cursor in the first field.
          autoFocus={autoFocus}
          disabled={disabled}
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy}
          className="border-console-border bg-console-bg text-console-text placeholder:text-console-muted aria-invalid:border-state-failed rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50"
        />
      )}

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
