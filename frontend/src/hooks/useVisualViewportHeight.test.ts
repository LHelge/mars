import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useVisualViewportHeight } from "./useVisualViewportHeight";

/** A `VisualViewport` whose height the test moves, firing an event. */
function fakeViewport(initial: number) {
  const listeners = new Map<string, Set<() => void>>();
  const viewport = {
    height: initial,
    addEventListener: vi.fn((type: string, listener: () => void) => {
      const set = listeners.get(type) ?? new Set();
      set.add(listener);
      listeners.set(type, set);
    }),
    removeEventListener: vi.fn((type: string, listener: () => void) => {
      listeners.get(type)?.delete(listener);
    }),
  };
  const fire = (type: string, height: number): void => {
    viewport.height = height;
    for (const listener of listeners.get(type) ?? []) listener();
  };
  const count = (): number =>
    [...listeners.values()].reduce((sum, set) => sum + set.size, 0);
  return { viewport, fire, count };
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("useVisualViewportHeight", () => {
  it("is undefined where visualViewport does not exist", () => {
    vi.stubGlobal("visualViewport", undefined);

    const { result } = renderHook(() => useVisualViewportHeight());

    expect(result.current).toBeUndefined();
  });

  it("reads the height in whole pixels and follows resize and scroll", () => {
    const fake = fakeViewport(839.4);
    vi.stubGlobal("visualViewport", fake.viewport);

    const { result } = renderHook(() => useVisualViewportHeight());
    expect(result.current).toBe(839);

    // The keyboard opens.
    act(() => {
      fake.fire("resize", 412.6);
    });
    expect(result.current).toBe(413);

    // iOS pans the visual viewport as the keyboard settles.
    act(() => {
      fake.fire("scroll", 400);
    });
    expect(result.current).toBe(400);
  });

  it("unsubscribes on unmount", () => {
    const fake = fakeViewport(700);
    vi.stubGlobal("visualViewport", fake.viewport);

    const { unmount } = renderHook(() => useVisualViewportHeight());
    expect(fake.count()).toBe(2);

    unmount();
    expect(fake.count()).toBe(0);
  });
});
