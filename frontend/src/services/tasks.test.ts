import { describe, expect, it } from "vitest";

import { taskStreamUrl } from "./tasks";

describe("taskStreamUrl", () => {
  const pid = "11111111-1111-4111-8111-111111111111";

  it("builds the first connection's URL with after=0", () => {
    expect(taskStreamUrl(pid, "abc", 0)).toBe(
      `/api/projects/${pid}/tasks/stream?token=abc&after=0`,
    );
  });

  it("defaults after to 0", () => {
    expect(taskStreamUrl(pid, "abc")).toBe(taskStreamUrl(pid, "abc", 0));
  });

  it("percent-encodes the base64url-adjacent characters of a token", () => {
    // A JWT is base64url and has neither, but a token is opaque to us: `+`
    // would arrive as a space and `/` would end the path segment.
    expect(taskStreamUrl(pid, "a+b/c=", 7)).toBe(
      `/api/projects/${pid}/tasks/stream?token=a%2Bb%2Fc%3D&after=7`,
    );
  });

  it("carries the last received sequence", () => {
    expect(taskStreamUrl(pid, "t", 42)).toContain("&after=42");
  });
});
