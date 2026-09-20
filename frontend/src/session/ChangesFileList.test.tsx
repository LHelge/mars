// The file list is what a reviewer reads first, so its counts have to be
// right: the totals across the diff, `bin` where git counted nothing, and the
// notice that the patch below is cut short while the counts are not.

import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Diff } from "../types";
import { ChangesFileList } from "./ChangesFileList";

function diff(overrides: Partial<Diff> = {}): Diff {
  return {
    base: "main",
    // Obviously fake fixture values (CLAUDE.md, rule 3).
    head: "00000000-0000-4000-8000-0000000000a1",
    merge_base: "1111111111111111111111111111111111111111",
    files: [
      { path: "src/one.ts", status: "M", additions: 12, deletions: 3 },
      { path: "assets/logo.png", status: "A", additions: 0, deletions: 0 },
      { path: "src/gone.ts", status: "D", additions: 0, deletions: 40 },
    ],
    patch: "",
    truncated: false,
    ...overrides,
  };
}

afterEach(cleanup);

describe("ChangesFileList", () => {
  it("adds the counts of every file up in its header", () => {
    render(<ChangesFileList diff={diff()} />);

    // The totals sit beside the file count, apart from the per-file counts.
    const header = screen.getByText("3 files").parentElement;
    expect(header).not.toBeNull();
    expect(within(header as HTMLElement).getByText("+12")).toBeDefined();
    expect(within(header as HTMLElement).getByText("−43")).toBeDefined();
  });

  it("writes `bin` instead of zeroes for a binary file", () => {
    render(
      <ChangesFileList
        diff={diff()}
        binaryPaths={new Set(["assets/logo.png"])}
      />,
    );

    expect(screen.getByText("bin")).toBeDefined();
    // The other file with no additions keeps its counts: only the patch says
    // which file git refused to diff.
    expect(screen.getByText("−40")).toBeDefined();
  });

  it("shows the truncation notice, which does not apply to the counts", () => {
    render(<ChangesFileList diff={diff({ truncated: true })} />);

    expect(
      screen.getByText("Patch truncated at 1 MiB; file counts are complete"),
    ).toBeDefined();
    expect(screen.getByText("3 files")).toBeDefined();
  });

  it("says what an empty diff was compared against", () => {
    render(<ChangesFileList diff={diff({ files: [], base: "origin/main" })} />);

    expect(screen.getByText("No changes against origin/main")).toBeDefined();
  });

  it("reports the path of a clicked row", () => {
    const onSelect = vi.fn();
    render(<ChangesFileList diff={diff()} onSelect={onSelect} />);

    fireEvent.click(screen.getByText("src/one.ts"));

    expect(onSelect).toHaveBeenCalledWith("src/one.ts");
  });
});
