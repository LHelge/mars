import { describe, expect, it } from "vitest";

import { roundLabel } from "./taskChrome";

describe("roundLabel", () => {
  it("says nothing until a second round", () => {
    expect(roundLabel(0, 5)).toBeNull();
    expect(roundLabel(1, 5)).toBeNull();
  });

  it("shows the round against the project's max_rounds", () => {
    expect(roundLabel(2, 5)).toBe("round 2/5");
    expect(roundLabel(5, 5)).toBe("round 5/5");
  });

  it("shows the round alone while the limit is unknown", () => {
    expect(roundLabel(3, undefined)).toBe("round 3");
  });
});
