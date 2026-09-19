import { describe, expect, it } from "vitest";
import { PASSWORD_LENGTH_MESSAGE, validatePassword } from "./password";

describe("validatePassword", () => {
  it("rejects anything shorter than ten characters", () => {
    expect(validatePassword("")).toBe(PASSWORD_LENGTH_MESSAGE);
    expect(validatePassword("123456789")).toBe(PASSWORD_LENGTH_MESSAGE);
  });

  it("accepts the boundaries", () => {
    expect(validatePassword("1234567890")).toBeNull();
    expect(validatePassword("x".repeat(128))).toBeNull();
  });

  it("rejects more than 128 UTF-16 code units", () => {
    expect(validatePassword("x".repeat(129))).toBe(PASSWORD_LENGTH_MESSAGE);
    // Astral characters count as two code units here; the server's own rule
    // is the authoritative one and may differ at the edge.
    expect(validatePassword("🚀".repeat(65))).toBe(PASSWORD_LENGTH_MESSAGE);
  });
});
