// The render bounds of the Changes panel (`SPEC.md`, "Frontend", "Changes
// panel"): nothing here is virtualised, so a patch that is large only in
// total collapses as a file that is large on its own does.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { Diff } from "../../types";
import { DiffBody } from "./DiffBody";

/** A file of `changed` added lines, as git spells it. */
function file(path: string, changed: number): string {
  const added = Array.from({ length: changed }, (_, i) => `+line ${String(i)}`);
  return [
    `diff --git a/${path} b/${path}`,
    "--- /dev/null",
    `+++ b/${path}`,
    `@@ -0,0 +1,${String(changed)} @@`,
    ...added,
    "",
  ].join("\n");
}

function diffOf(files: { path: string; changed: number }[]): Diff {
  return {
    base: "main",
    head: "session/00000000-0000-4000-8000-0000000000a1",
    merge_base: "0123456789abcdef0123456789abcdef01234567",
    files: files.map((one) => ({
      path: one.path,
      status: "A",
      additions: one.changed,
      deletions: 0,
    })),
    patch: files.map((one) => file(one.path, one.changed)).join(""),
    truncated: false,
  };
}

afterEach(cleanup);

describe("DiffBody", () => {
  it("expands a small patch and leaves each block open", () => {
    render(
      <DiffBody
        diff={diffOf([
          { path: "a.ts", changed: 10 },
          { path: "b.ts", changed: 10 },
        ])}
      />,
    );

    for (const block of screen.getAllByRole("button", { expanded: true })) {
      expect(block.textContent).toContain("changed");
    }
    expect(screen.queryByText(/every file starts collapsed/)).toBeNull();
  });

  it("collapses every file once the whole patch is past the budget", () => {
    // Eleven files of 200 lines: no file is past the per-file bound of 500,
    // but together they are past the patch's 2,000.
    const files = Array.from({ length: 11 }, (_, i) => ({
      path: `f${String(i)}.ts`,
      changed: 200,
    }));
    render(<DiffBody diff={diffOf(files)} />);

    expect(
      screen.getByText(/2200 changed lines across 11 files/),
    ).toBeDefined();
    expect(screen.queryAllByRole("button", { expanded: true })).toHaveLength(0);
    // Nothing is hidden: every file still has its header to open it.
    expect(screen.getAllByRole("button", { expanded: false })).toHaveLength(11);
  });
});
