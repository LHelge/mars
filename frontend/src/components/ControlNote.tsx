// A reason under a control: why a button is disabled, what a disabled action
// would need, said as a line of text rather than a tooltip (`SPEC.md`,
// "Frontend", "Mobile layout": a `title` is never the only place a fact
// lives, because a touch screen has no hover to show it).
//
// A form field already has this line: `FieldShell`'s hint. This is the same
// muted `text-xs` line for a control that is not a field — a row's `Remove`,
// a launch button — and its `id` is what the control names in its
// `aria-describedby`, so a screen reader reads the reason with the control.

import type { ReactNode } from "react";

export interface ControlNoteProps {
  /** Named by the control's `aria-describedby`. */
  id: string;
  children: ReactNode;
  className?: string;
}

export function ControlNote({ id, children, className }: ControlNoteProps) {
  return (
    <p id={id} className={`text-console-muted text-xs ${className ?? ""}`}>
      {children}
    </p>
  );
}
