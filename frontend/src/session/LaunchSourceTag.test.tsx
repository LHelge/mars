// What a session nobody launched reads as (`SPEC.md`, "Frontend", Launch
// source). The label table has its own test beside it; this is the rendering
// seam — what the tag actually puts on screen for each source, including the
// `schedule` one, which no live session can produce until the scheduler job
// lands.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LAUNCH_SOURCE } from "../utils/testIds";
import { LaunchSourceTag } from "./LaunchSourceTag";

afterEach(cleanup);

describe("LaunchSourceTag", () => {
  it("reads as launched by a schedule", () => {
    render(<LaunchSourceTag source="schedule" />);

    const tag = screen.getByTestId(LAUNCH_SOURCE);
    expect(tag.textContent).toContain("schedule");
    expect(tag.getAttribute("title")).toBe(
      "Launched on a schedule, not by a person",
    );
    // The chip alone would be a bare word to a screen reader.
    expect(tag.textContent).toContain("launched this session");
  });

  it("reads as launched by the dispatcher", () => {
    render(<LaunchSourceTag source="dispatcher" />);

    expect(screen.getByTestId(LAUNCH_SOURCE).textContent).toContain(
      "dispatcher",
    );
  });

  it("says nothing about a session a person launched", () => {
    render(<LaunchSourceTag source="user" />);

    expect(screen.queryByTestId(LAUNCH_SOURCE)).toBeNull();
  });
});
