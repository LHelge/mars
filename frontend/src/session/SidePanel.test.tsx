// Which panel a session opens on, over the real registry (`sidePanels.ts`).
//
// The rule being kept here is that nothing expensive is shown to somebody who
// did not ask for it: a session launched from the UI arrives `creating`, and
// the panel must not hand it the terminal — a lazy chunk and, a moment later,
// a `/bin/bash -l` in the container — merely because `Changes` is not offered
// yet.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Session, SessionState } from "../types";
import { SidePanel } from "./SidePanel";

vi.mock("./ChangesPanel", () => ({
  ChangesPanel: () => <p>changes panel</p>,
}));
vi.mock("./TasksPanel", () => ({ TasksPanel: () => <p>tasks panel</p> }));
vi.mock("./TerminalView", () => ({
  TerminalView: () => <p>terminal panel</p>,
}));

// Obviously fake fixture values (CLAUDE.md, rule 3).
const SESSION_ID = "00000000-0000-4000-8000-0000000000a1";

function session(state: SessionState): Session {
  return { id: SESSION_ID, state } as Session;
}

/** The selected tab's label, whoever selected it. */
function selected(): string | null {
  const tab = screen
    .getAllByRole("tab")
    .find((node) => node.getAttribute("aria-selected") === "true");
  return tab?.textContent ?? null;
}

beforeEach(() => {
  // A wide screen, so the panel starts open rather than as a rail.
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() })),
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("SidePanel", () => {
  it("opens a session being created on Tasks, never on the terminal", () => {
    render(<SidePanel session={session("creating")} />);

    expect(selected()).toBe("Tasks");
    expect(screen.queryByText("terminal panel")).toBeNull();
    // `Changes` has nothing to diff yet, so it is not offered at all.
    expect(screen.queryByRole("tab", { name: "Changes" })).toBeNull();
  });

  it("opens on Changes once the session has a branch", () => {
    const { rerender } = render(<SidePanel session={session("creating")} />);
    expect(selected()).toBe("Tasks");

    // The launch completed: the tab the panel shows is derived, so it follows.
    rerender(<SidePanel session={session("running")} />);
    expect(selected()).toBe("Changes");
    expect(screen.getByText("changes panel")).toBeDefined();
    expect(screen.queryByText("terminal panel")).toBeNull();
  });

  it("keeps the operator's own tab when the session's state moves", async () => {
    const { rerender } = render(<SidePanel session={session("running")} />);

    fireEvent.click(screen.getByRole("tab", { name: "Terminal" }));
    expect(selected()).toBe("Terminal");
    // The chunk is fetched now, on the first open of the tab, and not before.
    expect(await screen.findByText("terminal panel")).toBeDefined();

    rerender(<SidePanel session={session("parked")} />);
    expect(selected()).toBe("Terminal");
  });

  it("moves between tabs with the arrow keys", () => {
    render(<SidePanel session={session("running")} />);
    const tablist = screen.getByRole("tablist");

    expect(selected()).toBe("Changes");
    fireEvent.keyDown(tablist, { key: "ArrowRight" });
    expect(selected()).toBe("Tasks");
    fireEvent.keyDown(tablist, { key: "End" });
    expect(selected()).toBe("Terminal");
    // And wraps, which is how the pattern behaves.
    fireEvent.keyDown(tablist, { key: "ArrowRight" });
    expect(selected()).toBe("Changes");
  });

  it("names its panel with the tab that selected it", () => {
    render(<SidePanel session={session("running")} />);

    const tab = screen.getByRole("tab", { name: "Changes" });
    const panel = screen.getByRole("tabpanel");
    expect(panel.getAttribute("aria-labelledby")).toBe(tab.id);
    expect(tab.getAttribute("aria-controls")).toBe(panel.id);
    // One tab stop for the list: the selected tab, and no other.
    expect(tab.tabIndex).toBe(0);
    expect(screen.getByRole("tab", { name: "Tasks" }).tabIndex).toBe(-1);
  });
});
