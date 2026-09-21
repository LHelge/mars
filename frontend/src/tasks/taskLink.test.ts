import { describe, expect, it } from "vitest";

import { parseTaskNumber, taskPath } from "./taskLink";

// An obviously fake project id (`CLAUDE.md`, rule 3).
const PROJECT = "00000000-0000-4000-8000-000000000001";

describe("taskPath", () => {
  it("is the route the board links to", () => {
    expect(taskPath(PROJECT, 12)).toBe(`/projects/${PROJECT}/tasks/12`);
  });
});

describe("parseTaskNumber", () => {
  it("accepts a plain positive integer", () => {
    expect(parseTaskNumber("42")).toBe(42);
  });

  it("refuses anything that is not one", () => {
    for (const raw of ["", "0", "-1", "1.5", "#12", "12a", "abc", " 12"]) {
      expect(parseTaskNumber(raw)).toBeNull();
    }
  });
});
