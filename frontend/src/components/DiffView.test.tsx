// `SPEC.md`, "Frontend", "Transcript rendering" and "Mobile layout": a diff is
// unified below `sm`, with no side-by-side toggle there, and the choice made
// at `sm` and up survives a trip below it.

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DiffLine } from "../utils/diff";
import { DiffView } from "./DiffView";

const LINES: DiffLine[] = [
  { type: "context", text: "def main():", oldNo: 1, newNo: 1 },
  { type: "del", text: '    print("hello")', oldNo: 2 },
  { type: "add", text: '    print("hello, world")', newNo: 2 },
];

/** A `matchMedia` whose one answer the test moves, firing `change`. */
function stubWidth(wide: boolean) {
  const listeners = new Set<() => void>();
  const list = {
    matches: wide,
    addEventListener: (_type: string, listener: () => void) => {
      listeners.add(listener);
    },
    removeEventListener: (_type: string, listener: () => void) => {
      listeners.delete(listener);
    },
  };
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => list),
  );
  return (next: boolean): void => {
    list.matches = next;
    for (const listener of listeners) listener();
  };
}

/** The split panes draw no `+`/`-` marker column; unified draws one per row. */
function markers(): number {
  return screen.queryAllByText("+", { exact: true }).length;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("DiffView", () => {
  it("hides the side-by-side toggle below sm by its class", () => {
    render(<DiffView lines={LINES} />);

    const toggle = screen.getByRole("button", { name: "Side by side" });
    expect(toggle.className.split(" ")).toContain("max-sm:hidden");
  });

  it("stays unified below sm whatever was chosen", () => {
    // jsdom has no `matchMedia`, so this is the narrow layout.
    render(<DiffView lines={LINES} />);

    fireEvent.click(screen.getByRole("button", { name: "Side by side" }));

    expect(markers()).toBe(1);
  });

  it("splits from sm up and keeps the choice across a trip below it", () => {
    const setWide = stubWidth(true);
    render(<DiffView lines={LINES} />);
    expect(markers()).toBe(1);

    fireEvent.click(screen.getByRole("button", { name: "Side by side" }));
    expect(markers()).toBe(0);

    act(() => {
      setWide(false);
    });
    expect(markers()).toBe(1);

    act(() => {
      setWide(true);
    });
    expect(markers()).toBe(0);
    expect(
      screen
        .getByRole("button", { name: "Unified" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
  });
});
