import { describe, expect, it } from "vitest";
import type { LaunchSource } from "../types";
import { launchSourceLabel, launchSourceTitle } from "./launchSource";

const ALL: LaunchSource[] = ["user", "dispatcher", "schedule"];

describe("launchSourceLabel", () => {
  it("labels both unattended sources", () => {
    expect(launchSourceLabel("dispatcher")).toBe("dispatcher");
    expect(launchSourceLabel("schedule")).toBe("schedule");
  });

  it("labels nothing for a session a person launched", () => {
    expect(launchSourceLabel("user")).toBeNull();
    expect(launchSourceTitle("user")).toBeNull();
  });

  it("answers for every member of the union", () => {
    for (const source of ALL) {
      expect(launchSourceLabel(source)).not.toBeUndefined();
      expect(launchSourceTitle(source)).not.toBeUndefined();
    }
  });

  it("says who launched it, not that nobody did", () => {
    expect(launchSourceTitle("dispatcher")).toContain("dispatcher");
    expect(launchSourceTitle("schedule")).toContain("schedule");
  });
});
