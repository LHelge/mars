// The drawer's Escape rule, away from the drawer: what the key means is a
// decision over four booleans, and what counts as a draft is a question about
// a DOM subtree. The browser-level half — that the key really reaches the
// dialog, that focus is really trapped and really handed back — is
// `frontend/tests/tasks.spec.ts`.

import { beforeEach, describe, expect, it } from "vitest";

import { escapeAction, hasDraftText } from "./drawerEscape";

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
