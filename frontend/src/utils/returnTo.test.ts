import { describe, expect, it } from "vitest";
import { safeReturnTo } from "./returnTo";

describe("safeReturnTo", () => {
  it("accepts an application path with a search string", () => {
    expect(safeReturnTo("/projects/abc/tasks/42?x=1")).toBe(
      "/projects/abc/tasks/42?x=1",
    );
    expect(safeReturnTo("/")).toBe("/");
    expect(safeReturnTo("/sessions/00000000-0000-0000-0000-000000000001")).toBe(
      "/sessions/00000000-0000-0000-0000-000000000001",
    );
  });

  it("rejects anything that could leave the origin", () => {
    expect(safeReturnTo("//evil.example")).toBeNull();
    expect(safeReturnTo("https://evil.example")).toBeNull();
    expect(safeReturnTo("/\\evil.example")).toBeNull();
    expect(safeReturnTo("projects")).toBeNull();
    expect(safeReturnTo("/proj ects")).toBeNull();
  });

  it("rejects the unauthenticated routes themselves", () => {
    expect(safeReturnTo("/login")).toBeNull();
    expect(safeReturnTo("/login?next=/")).toBeNull();
    expect(safeReturnTo("/forgot-password")).toBeNull();
    expect(safeReturnTo("/invite/fake-invite-token")).toBeNull();
    expect(safeReturnTo("/reset-password/fake-reset-token")).toBeNull();
  });

  it("rejects non-strings and the empty string", () => {
    expect(safeReturnTo("")).toBeNull();
    expect(safeReturnTo(undefined)).toBeNull();
    expect(safeReturnTo(null)).toBeNull();
    expect(safeReturnTo(42)).toBeNull();
    expect(safeReturnTo({ from: "/projects" })).toBeNull();
  });
});
