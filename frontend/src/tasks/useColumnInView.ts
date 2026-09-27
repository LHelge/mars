// Which board column a phone is looking at (`SPEC.md`, "Frontend", "Mobile
// layout"): below `sm` the strip shows one column at a time, and the chip row
// above it marks that one.
//
// The answer comes from an `IntersectionObserver` rooted at the strip, never
// from `scrollLeft` arithmetic: the browser already knows which column the
// snap left in view, including after a swipe, a resize or a column appearing,
// and arithmetic over widths and gaps would have to be kept in step with the
// classes that set them.

import { useEffect, useState } from "react";
import type { RefObject } from "react";

/** How much of a column must be in view for it to be the one shown. */
export const IN_VIEW_THRESHOLD = 0.6;

/** The attribute naming a column's key, on the column and on its chip. */
export const COLUMN_KEY_ATTRIBUTE = "data-column-key";

/**
 * The key of the column in view in `strip`, among `keys` — the first column
 * until the observer has said otherwise, or where there is no observer (jsdom,
 * or `enabled` false at `sm` and wider) — and a setter for a chip tap to move
 * the mark at once rather than when the scroll has settled.
 */
export function useColumnInView(
  strip: RefObject<HTMLElement | null>,
  keys: readonly string[],
  enabled: boolean,
): [string | undefined, (key: string) => void] {
  const [inView, setInView] = useState<string | null>(null);
  // The observed set changes when the columns do, not when their cards do.
  const signature = keys.join("\n");

  useEffect(() => {
    const root = strip.current;
    if (
      !enabled ||
      root === null ||
      typeof IntersectionObserver === "undefined"
    ) {
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (
            !entry.isIntersecting ||
            entry.intersectionRatio < IN_VIEW_THRESHOLD
          ) {
            continue;
          }
          const key = entry.target.getAttribute(COLUMN_KEY_ATTRIBUTE);
          if (key !== null) setInView(key);
        }
      },
      { root, threshold: IN_VIEW_THRESHOLD },
    );
    for (const column of root.querySelectorAll(`[${COLUMN_KEY_ATTRIBUTE}]`)) {
      observer.observe(column);
    }
    return () => {
      observer.disconnect();
    };
  }, [strip, signature, enabled]);

  const current = inView !== null && keys.includes(inView) ? inView : keys[0];
  return [current, setInView];
}
