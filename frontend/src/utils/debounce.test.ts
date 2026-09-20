import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { debounce } from "./debounce";

describe("debounce", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("runs once after the delay", () => {
    const callback = vi.fn();
    const debounced = debounce(callback, 100);

    debounced.run();
    expect(callback).not.toHaveBeenCalled();

    vi.advanceTimersByTime(100);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("collapses a burst into the last call", () => {
    const callback = vi.fn();
    const debounced = debounce(callback, 100);

    debounced.run();
    vi.advanceTimersByTime(60);
    debounced.run();
    vi.advanceTimersByTime(60);
    expect(callback).not.toHaveBeenCalled();

    vi.advanceTimersByTime(40);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("runs again after a settled call", () => {
    const callback = vi.fn();
    const debounced = debounce(callback, 100);

    debounced.run();
    vi.advanceTimersByTime(100);
    debounced.run();
    vi.advanceTimersByTime(100);

    expect(callback).toHaveBeenCalledTimes(2);
  });

  it("cancel drops a pending run", () => {
    const callback = vi.fn();
    const debounced = debounce(callback, 100);

    debounced.run();
    debounced.cancel();
    vi.advanceTimersByTime(1000);

    expect(callback).not.toHaveBeenCalled();
  });

  it("cancel with nothing pending does nothing", () => {
    const callback = vi.fn();
    const debounced = debounce(callback, 100);

    expect(() => {
      debounced.cancel();
    }).not.toThrow();
    vi.advanceTimersByTime(1000);
    expect(callback).not.toHaveBeenCalled();
  });
});
