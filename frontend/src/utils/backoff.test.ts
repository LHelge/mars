import { afterEach, describe, expect, it, vi } from "vitest";

import { BACKOFF_MAX_MS, backoffDelay, BACKOFF_BASE_MS } from "./backoff";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("backoffDelay", () => {
  it("doubles from one second to the thirty second cap", () => {
    expect(backoffDelay(0, 0)).toBe(BACKOFF_BASE_MS);
    expect(backoffDelay(1, 0)).toBe(2000);
    expect(backoffDelay(4, 0)).toBe(16_000);
    expect(backoffDelay(5, 0)).toBe(BACKOFF_MAX_MS);
    expect(backoffDelay(40, 0)).toBe(BACKOFF_MAX_MS);
  });

  it("spreads the delay by the jitter fraction", () => {
    vi.spyOn(Math, "random").mockReturnValue(0);
    expect(backoffDelay(0)).toBe(800);
    vi.spyOn(Math, "random").mockReturnValue(1);
    expect(backoffDelay(0)).toBe(1200);
  });
});
