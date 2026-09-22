// A labelled text input: `FieldShell` plus the one control almost every form
// field actually is (`CLAUDE.md`, "Frontend conventions": shared UI).
//
// `value` and `onChange` are the input's, and no longer optional-in-practice:
// a field whose control is a select, a textarea or an output renders
// `FieldShell` directly and is not made to invent a setter it never calls.

import type { HTMLInputTypeAttribute } from "react";

import type { HelpTopic } from "../help/topics";
import { FieldShell } from "./FieldShell";
import { CONTROL } from "./fieldStyles";

export interface FormFieldProps {
  label: string;
  /** The control's `name` and its `id` (`FieldShell`); unique in the document. */
  name: string;
  type?: HTMLInputTypeAttribute;
  value: string;
  onChange: (value: string) => void;
  error?: string;
  hint?: string;
  /** The help topic a `Learn more` link after the hint opens (`FieldShell`). */
  help?: HelpTopic;
  /**
   * Passed through verbatim. A password field always says which password it is
   * — `current-password` or `new-password` — and never falls back to `on`.
   */
  autoComplete?: string;
  placeholder?: string;
  required?: boolean;
  autoFocus?: boolean;
  disabled?: boolean;
}

export function FormField({
  label,
  name,
  type = "text",
  value,
  onChange,
  error,
  hint,
  help,
  autoComplete,
  placeholder,
  required,
  autoFocus,
  disabled,
}: FormFieldProps) {
  return (
    <FieldShell
      label={label}
      name={name}
      error={error}
      hint={hint}
      help={help}
      required={required}
    >
      {(control) => (
        <input
          {...control}
          type={type}
          value={value}
          onChange={(event) => {
            onChange(event.target.value);
          }}
          autoComplete={autoComplete}
          placeholder={placeholder}
          required={required}
          // Login and invite pages put the cursor in the first field.
          autoFocus={autoFocus}
          disabled={disabled}
          className={CONTROL}
        />
      )}
    </FieldShell>
  );
}
