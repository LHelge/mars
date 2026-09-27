// What Escape, `Close` and a tap outside the panel mean inside the task drawer,
// and the seam a sub-form uses to claim Escape (`SPEC.md`, "Frontend", "Task
// board": the drawer's focus and close rules).
//
// The drawer is a native modal `<dialog>`, so focus containment, the inert
// background and the focus restore are the browser's. What is not the
// browser's is *when* Escape may close things: a key that dismissed an IME
// candidate list or an autocomplete popup must not also throw away eight lines
// of a description, and a sub-form that is open is what Escape should shut
// first. Both decisions are made here, in one pure function the drawer calls
// and a unit test drives directly — and a pointer's way out goes through the
// same function, so unsaved text is asked about however the drawer is left.
//
// This module holds no components on purpose: the drawer renders the context
// provider itself, so a module that ships a hook and a context can stay a
// `.ts` file (`CLAUDE.md`, "Frontend conventions").

import { createContext, useContext, useEffect, useRef } from "react";

/**
 * What the drawer should do with a request to close it: an Escape key press,
 * or a tap on `Close` or on the overlay beside the panel.
 */
export type CloseAction =
  /** Not ours: the key was already handled, or it is ending a composition. */
  | "ignore"
  /** There is unsaved text: ask first. Escape asks with a line and waits for
   *  a second press; a pointer asks with a confirmation panel. */
  | "confirm"
  /** For Escape, close the innermost open thing — a sub-form if there is one,
   *  else the drawer; for `Close` and the overlay, close the drawer. */
  | "close";

/** An Escape key press, as the drawer's document-level handler sees it. */
export interface EscapeRequest {
  via: "escape";
  /** Something nearer the key already called `preventDefault()`. */
  defaultPrevented: boolean;
  /** The key is ending an IME composition (`KeyboardEvent.isComposing`). */
  composing: boolean;
  /** The drawer holds text a user typed and has not saved. */
  dirty: boolean;
  /**
   * A question is already open: a previous Escape asked and nothing has been
   * typed since, or a pointer's confirmation panel is showing.
   */
  armed: boolean;
}

/** A tap on the header's `Close` or on the overlay beside the panel. */
export interface ButtonRequest {
  via: "button";
  /** The drawer holds text a user typed and has not saved. */
  dirty: boolean;
}

export type CloseRequest = EscapeRequest | ButtonRequest;

/**
 * The whole close rule of the drawer, as one decision, whichever way out the
 * user took (`SPEC.md`, "Frontend", "Task board").
 *
 * A dirty drawer costs one extra step, never a silent loss; a clean one closes
 * at once, exactly as it always did. Escape's extra step is a second press; a
 * pointer's is the `Discard and close` button of the panel that asks, which
 * closes without asking again.
 */
export function closeAction(request: CloseRequest): CloseAction {
  if (request.via === "button") return request.dirty ? "confirm" : "close";
  if (request.defaultPrevented || request.composing) return "ignore";
  if (request.dirty && !request.armed) return "confirm";
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
  const controls = root.querySelectorAll<
    HTMLInputElement | HTMLTextAreaElement
  >(DRAFT_CONTROLS);
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
  /**
   * The innermost open sub-form's `close`, or `undefined` when the drawer
   * itself is the innermost open thing.
   */
  innermost: () => (() => void) | undefined;
}

/**
 * One drawer's registry: a stack of the `close` of everything open under it.
 *
 * Deliberately outside React state — a registration is not a render — so the
 * key handler reads the stack as it is at the moment of the press, and the
 * ordering rule the drawer depends on is a plain function a unit test drives.
 */
export function createEscapeRegistry(): DrawerEscapeRegistry {
  let open: (() => void)[] = [];

  return {
    register(close) {
      open = [...open, close];
      return () => {
        open = open.filter((other) => other !== close);
      };
    },
    innermost: () => open.at(-1),
  };
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
 *
 * `open` is what the form's own `Cancel` is: a submission in flight owns the
 * form it was made from, so a form whose `Cancel` is shut while it sends does
 * not claim Escape either, and passes `!loading` here. The drawer may still
 * close over it — the request lands regardless — but no single key press takes
 * a pending form's fields out from under it.
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
