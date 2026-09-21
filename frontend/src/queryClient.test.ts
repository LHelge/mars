import { describe, expect, it } from "vitest";
import { shouldRetry } from "./queryClient";
import { ApiError } from "./services/apiClient";

describe("shouldRetry", () => {
  it("never retries an answer below 500", () => {
    for (const status of [400, 401, 403, 404, 409, 422, 429]) {
      expect(shouldRetry(0, new ApiError(status, "no"))).toBe(false);
    }
  });

  it("retries a 5xx once", () => {
    expect(shouldRetry(0, new ApiError(502, "bad gateway"))).toBe(true);
    expect(shouldRetry(1, new ApiError(502, "bad gateway"))).toBe(false);
  });

  it("retries a request that never reached the server once", () => {
    // `fetch` rejects with a `TypeError`; nothing answered, so it is worth one
    // more try.
    const offline = new TypeError("Failed to fetch");
    expect(shouldRetry(0, offline)).toBe(true);
    expect(shouldRetry(1, offline)).toBe(false);
  });
});
