import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useMediaQuery } from "./useMediaQuery";

/** A `MediaQueryList` whose `matches` the test moves, firing `change`. */
function fakeList(initial: boolean) {
  const listeners = new Set<() => void>();
  const list = {
    matches: initial,
    addEventListener: vi.fn((_type: string, listener: () => void) => {
      listeners.add(listener);
    }),
    removeEventListener: vi.fn((_type: string, listener: () => void) => {
      listeners.delete(listener);
    }),
  };
  const set = (matches: boolean): void => {
    list.matches = matches;
    for (const listener of listeners) listener();
  };
  return { list, set, listeners };
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("useMediaQuery", () => {
  it("is false where matchMedia does not exist", () => {
    vi.stubGlobal("matchMedia", undefined);

    const { result } = renderHook(() => useMediaQuery("(min-width: 64rem)"));

    expect(result.current).toBe(false);
  });

  it("reads the query and follows its change events", () => {
    const fake = fakeList(true);
    const matchMedia = vi.fn(() => fake.list);
    vi.stubGlobal("matchMedia", matchMedia);

    const { result, rerender } = renderHook(() =>
      useMediaQuery("(min-width: 64rem)"),
    );
    expect(result.current).toBe(true);
    expect(matchMedia).toHaveBeenCalledWith("(min-width: 64rem)");

    act(() => {
      fake.set(false);
    });
    expect(result.current).toBe(false);

    act(() => {
      fake.set(true);
    });
    expect(result.current).toBe(true);

    // One list per query: a re-render does not ask again.
    rerender();
    expect(matchMedia).toHaveBeenCalledTimes(1);
  });

  it("unsubscribes on unmount", () => {
    const fake = fakeList(false);
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => fake.list),
    );

    const { unmount } = renderHook(() => useMediaQuery("(min-width: 64rem)"));
    expect(fake.listeners.size).toBe(1);

    unmount();
    expect(fake.listeners.size).toBe(0);
  });
});
