// The drawer's Escape rule, away from the drawer: what the key means is a
// decision over four booleans, what counts as a draft is a question about a
// DOM subtree, and which of the drawer's half-dozen sub-forms a press belongs
// to is a stack. The browser-level half — that the key really reaches the
// dialog, that focus is really trapped and really handed back — is
// `frontend/tests/tasks.spec.ts`.

import { beforeEach, describe, expect, it } from "vitest";

import {
  createEscapeRegistry,
  escapeAction,
  hasDraftText,
} from "./drawerEscape";

describe("escapeAction", () => {
  const clean = {
    defaultPrevented: false,
    composing: false,
    dirty: false,
    armed: false,
  };

  it("closes on the first press when nothing is unsaved", () => {
    expect(escapeAction(clean)).toBe("close");
  });

  it("leaves a key another handler already took", () => {
    expect(escapeAction({ ...clean, defaultPrevented: true })).toBe("ignore");
  });

  it("leaves a key that is ending a composition", () => {
    // The Escape that dismisses an IME candidate list is not a close request,
    // and neither is the one that dismisses an autocomplete popup above.
    expect(escapeAction({ ...clean, composing: true })).toBe("ignore");
    expect(escapeAction({ ...clean, composing: true, dirty: true })).toBe(
      "ignore",
    );
  });

  it("asks before discarding a draft, and closes on the second press", () => {
    expect(escapeAction({ ...clean, dirty: true })).toBe("confirm");
    expect(escapeAction({ ...clean, dirty: true, armed: true })).toBe("close");
  });

  it("does not ask twice about a drawer with nothing in it", () => {
    expect(escapeAction({ ...clean, armed: true })).toBe("close");
  });
});

describe("createEscapeRegistry", () => {
  it("has nothing to close until a sub-form registers", () => {
    expect(createEscapeRegistry().innermost()).toBeUndefined();
  });

  it("closes the innermost sub-form first, one press at a time", () => {
    const registry = createEscapeRegistry();
    const closed: string[] = [];
    const closeForm = registry.register(() => closed.push("form"));
    const closeConfirmation = registry.register(() =>
      closed.push("confirmation"),
    );

    // A confirmation opened inside a form is what Escape shuts first, and the
    // form only once the confirmation has unregistered itself.
    registry.innermost()?.();
    closeConfirmation();
    registry.innermost()?.();
    closeForm();

    expect(closed).toEqual(["confirmation", "form"]);
    // Everything under the drawer is shut, so the next press is the drawer's.
    expect(registry.innermost()).toBeUndefined();
  });

  it("leaves the forms still open when one in the middle closes", () => {
    // A form can go for a reason of its own — a save landing, a launch
    // starting — and taking the wrong one off the stack would leave Escape
    // calling a `close` for something that is no longer on screen.
    const registry = createEscapeRegistry();
    const closed: string[] = [];
    registry.register(() => closed.push("outer"));
    const closeMiddle = registry.register(() => closed.push("middle"));
    registry.register(() => closed.push("inner"));

    closeMiddle();
    registry.innermost()?.();

    expect(closed).toEqual(["inner"]);
  });
});

describe("hasDraftText", () => {
  let root: HTMLElement;

  beforeEach(() => {
    root = document.createElement("div");
    document.body.append(root);
  });

  function html(markup: string): boolean {
    root.innerHTML = markup;
    return hasDraftText(root);
  }

  it("is false for an empty drawer", () => {
    expect(html("<textarea></textarea><input type='text' />")).toBe(false);
  });

  it("is false for whitespace alone", () => {
    root.innerHTML = "<textarea></textarea>";
    root.querySelector("textarea")!.value = "   \n ";
    expect(hasDraftText(root)).toBe(false);
  });

  it("finds text in a textarea and in a plain input", () => {
    root.innerHTML = "<textarea></textarea>";
    root.querySelector("textarea")!.value = "Two steps, both reversible.";
    expect(hasDraftText(root)).toBe(true);

    root.innerHTML = "<input />";
    root.querySelector("input")!.value = "Draft the migration plan";
    expect(hasDraftText(root)).toBe(true);
  });

  it("ignores controls that cannot hold a draft", () => {
    // A select or a checkbox loses nothing a user wrote, and a disabled or
    // read-only field is not being written in at all.
    expect(
      html(
        "<select><option selected>ready</option></select>" +
          "<input type='checkbox' checked />" +
          "<input type='submit' value='Save changes' />",
      ),
    ).toBe(false);

    root.innerHTML = "<textarea disabled></textarea><input readonly />";
    root.querySelector("textarea")!.value = "sending…";
    root.querySelector("input")!.value = "abc1234";
    expect(hasDraftText(root)).toBe(false);
  });
});
