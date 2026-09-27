// The one place JavaScript reads the viewport (`ARCHITECTURE.md`, "Frontend
// architecture", One layout, adapted at the edges).
//
// Layout is class names: a breakpoint prefix restyles what is already mounted.
// This hook is for the other case, where the width decides *what* is mounted or
// what a component shows, and it has to follow a resize across the breakpoint
// rather than read the width once on the first render.

import { useCallback, useMemo, useSyncExternalStore } from "react";

/** Tailwind's `lg` (1024 px) as a media query: the side panel's breakpoint. */
export const LG_QUERY = "(min-width: 64rem)";

/**
 * Tailwind's `sm` (640 px) as a media query: the composer's height cap, and
 * below it the task board mounts its column picker and the drawer's `Copy
 * link` moves into its action bar.
 */
export const SM_QUERY = "(min-width: 40rem)";

/**
 * A touch screen as the primary pointer — Tailwind's `pointer-coarse:` in
 * JavaScript. Touch is read from the pointer, never from the width (ADR 0057).
 */
export const COARSE_QUERY = "(pointer: coarse)";

/**
 * Whether `query` matches now, re-rendering when that changes.
 *
 * `false` where `window.matchMedia` does not exist — jsdom, and any render
 * without a window — so a component falls back to its narrow layout there.
 */
export function useMediaQuery(query: string): boolean {
  const list = useMemo(
    () =>
      typeof window !== "undefined" && typeof window.matchMedia === "function"
        ? window.matchMedia(query)
        : undefined,
    [query],
  );

  const subscribe = useCallback(
    (onChange: () => void) => {
      if (list === undefined) return () => undefined;
      list.addEventListener("change", onChange);
      return () => {
        list.removeEventListener("change", onChange);
      };
    },
    [list],
  );

  return useSyncExternalStore(
    subscribe,
    () => list?.matches ?? false,
    () => false,
  );
}
