import { describe, expect, it } from "vitest";
import {
  SECRET_NAME_MESSAGE,
  SECRET_NAME_RE,
  validateSecretName,
} from "./secretName";

describe("validateSecretName", () => {
  it("accepts an environment-variable name", () => {
    expect(validateSecretName("MY_TOKEN_2")).toBeNull();
    expect(validateSecretName("A")).toBeNull();
    expect(validateSecretName(`A${"B".repeat(127)}`)).toBeNull();
  });

  it("rejects anything the column would reject", () => {
    expect(validateSecretName("lowercase")).toBe(SECRET_NAME_MESSAGE);
    expect(validateSecretName("1ABC")).toBe(SECRET_NAME_MESSAGE);
    expect(validateSecretName("_LEADING")).toBe(SECRET_NAME_MESSAGE);
    expect(validateSecretName("WITH-DASH")).toBe(SECRET_NAME_MESSAGE);
    expect(validateSecretName("")).toBe(SECRET_NAME_MESSAGE);
    // 129 characters: one past the column's limit.
    expect(validateSecretName(`A${"B".repeat(128)}`)).toBe(SECRET_NAME_MESSAGE);
  });

  it("is the pattern of `docs/data-model.md`", () => {
    expect(SECRET_NAME_RE.source).toBe("^[A-Z][A-Z0-9_]{0,127}$");
  });
});
