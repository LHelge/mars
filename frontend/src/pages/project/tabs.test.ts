import { describe, expect, it } from "vitest";

import { DEFAULT_PROJECT_TAB, PROJECT_TABS, parseProjectTab } from "./tabs";

describe("project tabs", () => {
  it("orders the strip with branches right after the board", () => {
    expect(PROJECT_TABS.map((entry) => entry.value)).toEqual([
      "sessions",
      "board",
      "branches",
      "profiles",
      "shared-dirs",
      "states",
      "secrets",
    ]);
  });

  it("labels the branches tab as the git panel's header does", () => {
    expect(
      PROJECT_TABS.find((entry) => entry.value === "branches")?.label,
    ).toBe("Branches");
  });

  it("parses every tab value, branches included", () => {
    expect(parseProjectTab("branches")).toBe("branches");
    for (const entry of PROJECT_TABS) {
      expect(parseProjectTab(entry.value)).toBe(entry.value);
    }
  });

  it("falls back to the sessions tab for a missing or unknown value", () => {
    expect(parseProjectTab(null)).toBe(DEFAULT_PROJECT_TAB);
    expect(parseProjectTab("git")).toBe("sessions");
  });
});
