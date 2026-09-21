import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { coalesce } from "./coalesce";

describe("coalesce", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("runs once at the end of the tick", () => {
    const callback = vi.fn();
    const coalesced = coalesce(callback);

    coalesced.run();
    expect(callback).not.toHaveBeenCalled();

    vi.advanceTimersByTime(0);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("collapses a burst into one run", () => {
    const callback = vi.fn();
    const coalesced = coalesce(callback);

    for (let i = 0; i < 50; i += 1) coalesced.run();

    vi.advanceTimersByTime(0);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("does not starve under calls that never stop", () => {
    const callback = vi.fn();
    const coalesced = coalesce(callback, 10);

    // The difference from `debounce`: a caller asking again every tick keeps
    // restarting a debounce's timer and would never run at all.
    for (let tick = 0; tick < 5; tick += 1) {
      coalesced.run();
      vi.advanceTimersByTime(6);
      coalesced.run();
      vi.advanceTimersByTime(6);
    }

    expect(callback.mock.calls.length).toBeGreaterThanOrEqual(5);
  });

  it("runs again after a settled run", () => {
    const callback = vi.fn();
    const coalesced = coalesce(callback);

    coalesced.run();
    vi.advanceTimersByTime(0);
    coalesced.run();
    vi.advanceTimersByTime(0);

    expect(callback).toHaveBeenCalledTimes(2);
  });

  it("cancel drops a pending run, and cancelling nothing is safe", () => {
    const callback = vi.fn();
    const coalesced = coalesce(callback);

    coalesced.run();
    coalesced.cancel();
    coalesced.cancel();
    vi.advanceTimersByTime(1000);

    expect(callback).not.toHaveBeenCalled();
  });
});
