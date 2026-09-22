// `CLAUDE.md`, "Frontend conventions", shared UI: the app's one button has to
// be able to say what it is — a disclosure toggle, a button disabled for a
// reason — without a wrapper the keyboard cannot reach.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Icon } from "./icons";
import { SubmitButton } from "./SubmitButton";

afterEach(() => {
  cleanup();
});

describe("SubmitButton", () => {
  it("passes native button attributes through", () => {
    render(
      <SubmitButton
        type="button"
        aria-expanded={false}
        title="default profile"
        data-testid="toggle-uses"
      >
        Uses
      </SubmitButton>,
    );

    const button = screen.getByRole("button", { name: "Uses" });
    expect(button.getAttribute("type")).toBe("button");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(button.getAttribute("title")).toBe("default profile");
    expect(button.getAttribute("data-testid")).toBe("toggle-uses");
  });

  it("draws its icon hidden, so the label alone names the button", () => {
    render(
      <SubmitButton type="button" icon={Icon.push}>
        Push
      </SubmitButton>,
    );

    const button = screen.getByRole("button", { name: "Push" });
    const svg = button.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
  });

  it("is not loading and not busy unless it is told to be", () => {
    render(<SubmitButton>Save</SubmitButton>);

    const button = screen.getByRole("button", { name: "Save" });
    expect((button as HTMLButtonElement).disabled).toBe(false);
    expect(button.getAttribute("aria-busy")).toBeNull();
    expect(button.getAttribute("type")).toBe("submit");
  });

  it("disables itself and says it is busy while loading", () => {
    render(<SubmitButton loading>Save</SubmitButton>);

    const button = screen.getByRole("button", { name: "Save" });
    expect((button as HTMLButtonElement).disabled).toBe(true);
    expect(button.getAttribute("aria-busy")).toBe("true");
  });
});
