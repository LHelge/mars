// Which panel a session opens on, over the real registry (`sidePanels.ts`).
//
// The rule being kept here is that nothing expensive is shown to somebody who
// did not ask for it: a session launched from the UI arrives `creating`, and
// the panel must not hand it the terminal — a lazy chunk and, a moment later,
// a `/bin/bash -l` in the container — merely because `Changes` is not offered
// yet.

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Session, SessionState } from "../types";
import { panelOpenerId, setPanelSheet } from "./sessionUi";
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

/** The viewport's side of `lg`, and the `change` it fires when it crosses. */
let wide = true;
const listeners = new Set<() => void>();

function resize(toWide: boolean): void {
  act(() => {
    wide = toWide;
    for (const listener of listeners) listener();
  });
}

beforeEach(() => {
  // A wide screen, so the panel starts open rather than as a rail.
  wide = true;
  listeners.clear();
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({
      get matches() {
        return wide;
      },
      addEventListener: (_type: string, listener: () => void) => {
        listeners.add(listener);
      },
      removeEventListener: (_type: string, listener: () => void) => {
        listeners.delete(listener);
      },
    })),
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

  it("offers no rail where matchMedia does not exist", () => {
    vi.stubGlobal("matchMedia", undefined);
    render(<SidePanel session={session("running")} />);

    // The narrow layout: a closed sheet, opened from the session header.
    expect(
      screen.queryByRole("button", { name: "Show side panel" }),
    ).toBeNull();
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("follows a resize across lg", () => {
    wide = false;
    render(<SidePanel session={session("running")} />);
    expect(screen.queryByRole("tablist")).toBeNull();
    // Below lg there is no rail: the opener is the header's.
    expect(
      screen.queryByRole("button", { name: "Show side panel" }),
    ).toBeNull();

    resize(true);
    expect(screen.getByRole("tablist")).toBeDefined();

    resize(false);
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("keeps the operator's hide until the width crosses lg again", () => {
    render(<SidePanel session={session("running")} />);

    fireEvent.click(screen.getByRole("button", { name: "Hide side panel" }));
    expect(screen.queryByRole("tablist")).toBeNull();

    // Narrow, then wide again: the crossing hands the choice back to the width.
    resize(false);
    expect(screen.queryByRole("tablist")).toBeNull();
    resize(true);
    expect(screen.getByRole("tablist")).toBeDefined();
  });

  describe("below lg, as a sheet", () => {
    beforeEach(() => {
      wide = false;
    });

    /** The header's opener, which the sheet hands the focus back to. */
    function renderWithOpener(state: SessionState = "running"): void {
      render(
        <>
          <button type="button" id={panelOpenerId(SESSION_ID)}>
            Panels
          </button>
          <SidePanel session={session(state)} />
        </>,
      );
    }

    function openSheet(): void {
      act(() => {
        setPanelSheet(SESSION_ID, true);
      });
    }

    it("opens as a dialog holding the tab list, focused on the selected tab", () => {
      renderWithOpener("creating");
      expect(screen.queryByRole("dialog")).toBeNull();

      openSheet();
      const dialog = screen.getByRole("dialog", { name: "Session panels" });
      expect(dialog.querySelector('[role="tablist"]')).not.toBeNull();
      // The derived-tab rule holds in the sheet too.
      expect(selected()).toBe("Tasks");
      expect(screen.queryByText("terminal panel")).toBeNull();
      expect(document.activeElement).toBe(
        screen.getByRole("tab", { name: "Tasks" }),
      );
    });

    it("closes on Escape and hands the focus back to the opener", () => {
      renderWithOpener();
      openSheet();

      fireEvent.keyDown(screen.getByRole("tab", { name: "Changes" }), {
        key: "Escape",
      });
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Panels" }),
      );
    });

    it("closes on its close button and on a tap on the overlay", () => {
      renderWithOpener();
      openSheet();
      fireEvent.click(screen.getByRole("button", { name: "Close panels" }));
      expect(screen.queryByRole("dialog")).toBeNull();

      openSheet();
      const overlay = screen.getByRole("dialog").previousElementSibling;
      expect(overlay).not.toBeNull();
      if (overlay !== null) fireEvent.click(overlay);
      expect(screen.queryByRole("dialog")).toBeNull();
    });

    it("is closed by a crossing of lg, in either direction", () => {
      renderWithOpener();
      openSheet();

      // Wide: the column the width wants, and no sheet left behind.
      resize(true);
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(screen.getByRole("tablist")).toBeDefined();

      // Narrow again: the sheet stays closed until it is asked for.
      resize(false);
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(screen.queryByRole("tablist")).toBeNull();
    });
  });
});
