import { describe, expect, it } from "vitest";

// Proves the Vitest runner works; real tests arrive with the stores and
// services that need them.
describe("vitest", () => {
  it("runs unit tests in a jsdom environment", () => {
    expect(typeof document).toBe("object");
  });
});
