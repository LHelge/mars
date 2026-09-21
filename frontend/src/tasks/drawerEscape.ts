// What Escape means inside the task drawer, and the seam a sub-form uses to
// claim it (`SPEC.md`, "Frontend", "Task board": the drawer's focus and
// Escape rules).
//
// The drawer is a native modal `<dialog>`, so focus containment, the inert
// background and the focus restore are the browser's. What is not the
// browser's is *when* Escape may close things: a key that dismissed an IME
// candidate list or an autocomplete popup must not also throw away eight lines
// of a description, and a sub-form that is open is what Escape should shut
// first. Both decisions are made here, in one pure function the drawer calls
// and a unit test drives directly.
//
// This module holds no components on purpose: the drawer renders the context
// provider itself, so a module that ships a hook and a context can stay a
// `.ts` file (`CLAUDE.md`, "Frontend conventions").

import { createContext, useContext, useEffect, useRef } from "react";

/** What the drawer should do with an Escape key press. */
export type EscapeAction =
  /** Not ours: the key was already handled, or it is ending a composition. */
  | "ignore"
  /** There is unsaved text: say so and wait for a second Escape. */
  | "confirm"
  /** Close the innermost open thing — a sub-form if there is one, else the
   *  drawer. */
  | "close";

export interface EscapeInput {
  /** Something nearer the key already called `preventDefault()`. */
  defaultPrevented: boolean;
  /** The key is ending an IME composition (`KeyboardEvent.isComposing`). */
  composing: boolean;
  /** The drawer holds text a user typed and has not saved. */
  dirty: boolean;
  /** A previous Escape already asked, and nothing has been typed since. */
  armed: boolean;
}

/**
 * The whole Escape rule of the drawer, as one decision.
 *
 * A dirty drawer costs one extra key press, never a silent loss; a clean one
 * closes on the first Escape exactly as it always did.
 */
export function escapeAction({
  defaultPrevented,
  composing,
  dirty,
  armed,
}: EscapeInput): EscapeAction {
  if (defaultPrevented || composing) return "ignore";
  if (dirty && !armed) return "confirm";
  return "close";
}

/**
 * The text controls of `root` that could hold a draft, in document order.
 *
 * Buttons, checkboxes and selects are not drafts — losing one loses nothing a
 * user wrote — and a disabled or read-only control is not one either.
 */
const DRAFT_CONTROLS = [
  "textarea",
  "input:not([type])",
  'input[type="text"]',
  'input[type="search"]',
  'input[type="url"]',
  'input[type="email"]',
  'input[type="number"]',
].join(", ");

/**
 * Does anything inside `root` hold text?
 *
 * Deliberately a DOM question rather than a React one: the forms under the
 * drawer belong to half a dozen modules, and the drawer should not need a
 * report from each of them to know that something is half-written. It is read
 * together with "has anyone typed in here at all" — a pre-filled edit form the
 * user has not touched is not a draft — which the drawer tracks from the
 * `input` events that bubble out of it.
 */
export function hasDraftText(root: ParentNode): boolean {
  const controls = root.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>(
    DRAFT_CONTROLS,
  );
  for (const control of controls) {
    if (control.disabled || control.readOnly) continue;
    if (control.value.trim() !== "") return true;
  }
  return false;
}

/**
 * The drawer's offer to its contents: "tell me how to close you, and Escape
 * will close you before it closes me".
 *
 * A sub-form registers while it is open and unregisters when it is not; the
 * most recently registered one wins, so a confirmation opened inside a form
 * closes before the form does.
 */
export interface DrawerEscapeRegistry {
  register: (close: () => void) => () => void;
}

export const DrawerEscapeContext = createContext<DrawerEscapeRegistry | null>(
  null,
);

/**
 * Claim Escape for a sub-form while `open` is true.
 *
 * One line in the form that owns the state:
 * `useDrawerEscape(() => { setOpen(false); }, open);`
 * Outside the drawer the context is absent and the hook does nothing, so a
 * form used elsewhere needs no condition around it.
 */
export function useDrawerEscape(close: () => void, open: boolean): void {
  const registry = useContext(DrawerEscapeContext);
  const latest = useRef(close);

  useEffect(() => {
    latest.current = close;
  }, [close]);

  useEffect(() => {
    if (registry === null || !open) return;
    return registry.register(() => {
      latest.current();
    });
  }, [registry, open]);
}
