// "Are you sure?", once, for the whole console (`SPEC.md`, "Frontend",
// Confirmations).
//
// This used to be five idioms: `window.confirm`, a button that turned into
// `Confirm delete` when pressed once, a warning `Alert` with two buttons in
// it, and two hand-rolled panels in the task drawer. `window.confirm` was the
// worst of them — unstyled, blocking the whole tab, impossible to say anything
// in beyond one line of plain text, and needing a dialog handler in every
// Playwright scenario that walked past it.
//
// What replaces them is an inline panel in the console's own language: it
// opens where the action was, in the tone of what it is about to do, and it
// says what will happen in a sentence that names the thing. Nothing is
// blocked while it is open — a running session goes on updating behind it —
// and the page can still be read, which is the point of asking at all.
//
// The panel is not a dialog and does not take focus. Inside the task drawer,
// where Escape has a meaning, the caller registers its own `onCancel` with
// `useDrawerEscape` so a key press withdraws the confirmation before it closes
// the drawer (`tasks/drawerEscape.ts`).
//
// `pending` and `error` come from whatever owns the write — a `useMutation`'s
// `isPending` and `errorMessage(mutation.error)`, or a `useFormSubmit`'s
// `loading` and `error` (`CLAUDE.md`, "Frontend conventions", Submitting a
// form). The panel keeps no state of its own at all.

import type { ReactNode } from "react";

import { Alert } from "./Alert";
import { SubmitButton } from "./SubmitButton";

export type ConfirmTone =
  /** Something is destroyed, and cannot be brought back. */
  | "danger"
  /** Something is given up or overwritten: recoverable, but worth saying. */
  | "caution";

const TONES: Record<ConfirmTone, string> = {
  danger: "border-state-failed/60",
  caution: "border-state-human/60",
};

export interface ConfirmPanelProps {
  tone?: ConfirmTone;
  /** What is about to happen, naming the thing it happens to. */
  message: ReactNode;
  /**
   * What the confirmation asks for besides a yes, such as the comment a drop
   * of a task's hand-off requires; rendered between the message and the
   * refusal, and owned by the caller like everything else here.
   */
  children?: ReactNode;
  /**
   * The confirming button's label, and so its accessible name: it names its
   * target — `Delete user ada`, not `Delete` — because a table of rows offers
   * the same verb a dozen times over.
   */
  confirmLabel: string;
  /** The way out, when "Cancel" is not the clearest word for it. */
  cancelLabel?: string;
  /** The confirmed write is in flight. */
  pending?: boolean;
  /** What the last attempt was refused with, shown inside the panel. */
  error?: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmPanel({
  tone = "danger",
  message,
  children,
  confirmLabel,
  cancelLabel = "Cancel",
  pending = false,
  error = null,
  onConfirm,
  onCancel,
}: ConfirmPanelProps) {
  return (
    <div
      className={`bg-console-bg flex flex-col gap-2 rounded border p-2 ${TONES[tone]}`}
    >
      <p className="text-console-text max-w-prose text-sm">{message}</p>

      {children}

      {error !== null && <Alert kind="error">{error}</Alert>}

      <div className="flex flex-wrap justify-end gap-2">
        <SubmitButton
          type="button"
          variant="ghost"
          disabled={pending}
          onClick={onCancel}
        >
          {cancelLabel}
        </SubmitButton>
        <SubmitButton
          type="button"
          variant={tone === "danger" ? "danger" : "primary"}
          loading={pending}
          onClick={onConfirm}
        >
          {confirmLabel}
        </SubmitButton>
      </div>
    </div>
  );
}
