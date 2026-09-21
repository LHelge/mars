// `CLAUDE.md`, "Frontend conventions", shared UI: a field's label, hint and
// error have to reach its control whatever the control is, and two fields of
// the same name on one screen have to stay apart.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { FieldShell } from "./FieldShell";
import { FormField } from "./FormField";

afterEach(() => {
  cleanup();
});

/** The ids a control says describe it, in document order. */
function describedBy(control: HTMLElement): string[] {
  const value = control.getAttribute("aria-describedby");
  return value === null ? [] : value.split(" ");
}

function textOf(id: string): string | null {
  return document.getElementById(id)?.textContent ?? null;
}

describe("FieldShell", () => {
  it("labels a custom control and connects its hint and error", () => {
    render(
      <FieldShell
        label="Move to"
        name="review-state"
        hint="Where the task goes."
        error="Choose a state"
      >
        {(control) => (
          <select {...control}>
            <option value="">Choose a state</option>
          </select>
        )}
      </FieldShell>,
    );

    const select = screen.getByLabelText("Move to");
    expect(select.tagName).toBe("SELECT");
    expect(select.getAttribute("name")).toBe("review-state");
    expect(select.getAttribute("aria-invalid")).toBe("true");

    const ids = describedBy(select);
    expect(ids).toHaveLength(2);
    expect(ids.map(textOf)).toEqual(["Choose a state", "Where the task goes."]);
  });

  it("names only the hint when there is no error", () => {
    render(
      <FieldShell label="Commit" name="handoff-commit" hint="A full sha.">
        {(control) => <input {...control} />}
      </FieldShell>,
    );

    const input = screen.getByLabelText("Commit");
    expect(input.getAttribute("aria-invalid")).toBeNull();
    expect(describedBy(input).map(textOf)).toEqual(["A full sha."]);
  });

  it("describes nothing when there is neither hint nor error", () => {
    render(
      <FieldShell label="Target" name="merge-target">
        {(control) => <input {...control} />}
      </FieldShell>,
    );

    expect(
      screen.getByLabelText("Target").getAttribute("aria-describedby"),
    ).toBeNull();
  });

  it("marks a required field on its label, not on the accessible name", () => {
    render(
      <FieldShell label="Value" name="secret-value" required>
        {(control) => <textarea {...control} />}
      </FieldShell>,
    );

    expect(screen.getByLabelText("Value *").tagName).toBe("TEXTAREA");
  });

  it("keeps the control's id and derives the hint and error ids from it", () => {
    // Two forms of the same shape are told apart by their names, which is
    // also how an end-to-end selector reaches one form's control.
    render(
      <FieldShell
        label="Target"
        name="git-merge-abc-target"
        hint="Where it lands."
        error="No integration head"
      >
        {(control) => <input {...control} />}
      </FieldShell>,
    );

    const input = screen.getByLabelText("Target");
    expect(input.id).toBe("git-merge-abc-target");
    expect(describedBy(input)).toEqual([
      "git-merge-abc-target-error",
      "git-merge-abc-target-hint",
    ]);
  });
});

describe("FormField", () => {
  it("renders an input its label points at, with hint and error attached", () => {
    render(
      <FormField
        label="Labels"
        name="task-labels"
        value="docs"
        onChange={() => undefined}
        hint="Separated by spaces."
        error="One label is too long"
      />,
    );

    const input = screen.getByLabelText("Labels");
    expect(input.tagName).toBe("INPUT");
    expect((input as HTMLInputElement).value).toBe("docs");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(describedBy(input).map(textOf)).toEqual([
      "One label is too long",
      "Separated by spaces.",
    ]);
  });
});
