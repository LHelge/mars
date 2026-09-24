import { describe, expect, it } from "vitest";

import type { Branch } from "../../types";
import { integrationHeads } from "./integrationHeads";

const BRANCHES: Branch[] = [
  { name: "release", kind: "head", commit: "r1" },
  { name: "origin/main", kind: "upstream", commit: "u1" },
  { name: "main", kind: "head", commit: "m1" },
  { name: "develop", kind: "head", commit: "d1" },
  {
    name: "refs/sessions/5f0c",
    kind: "session",
    commit: "s1",
    session_id: "5f0c",
  },
];

describe("integrationHeads", () => {
  it("lists heads alone, the default first and the rest by name", () => {
    expect(integrationHeads(BRANCHES, "main").map((head) => head.name)).toEqual(
      ["main", "develop", "release"],
    );
  });

  it("marks the default head and pairs each head with its upstream", () => {
    const [main, develop] = integrationHeads(BRANCHES, "main");
    expect(main).toEqual({
      name: "main",
      commit: "m1",
      isDefault: true,
      upstream: "u1",
    });
    expect(develop).toMatchObject({ isDefault: false, upstream: null });
  });

  it("marks nothing when the project has no default branch", () => {
    expect(integrationHeads(BRANCHES, null).some((head) => head.isDefault)).toBe(
      false,
    );
  });
});
