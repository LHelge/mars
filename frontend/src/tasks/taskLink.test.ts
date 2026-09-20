import { describe, expect, it } from "vitest";

import { buildTaskLink, parseTaskNumber, taskPath } from "./taskLink";

// An obviously fake project id (`CLAUDE.md`, rule 3).
const PROJECT = "00000000-0000-4000-8000-000000000001";

describe("taskPath", () => {
  it("is the route the board links to", () => {
    expect(taskPath(PROJECT, 12)).toBe(`/projects/${PROJECT}/tasks/12`);
  });
});

describe("buildTaskLink", () => {
  it("joins the origin to the canonical path", () => {
    expect(buildTaskLink("https://mars.example", PROJECT, 12)).toBe(
      `https://mars.example/projects/${PROJECT}/tasks/12`,
    );
  });

  it("does not double the separator when the origin ends in a slash", () => {
    expect(buildTaskLink("https://mars.example/", PROJECT, 7)).toBe(
      `https://mars.example/projects/${PROJECT}/tasks/7`,
    );
  });

  it("carries no query string and no fragment", () => {
    const link = buildTaskLink("https://mars.example", PROJECT, 3);
    expect(link).not.toContain("?");
    expect(link).not.toContain("#");
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
