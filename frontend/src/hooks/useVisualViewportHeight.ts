// The height of the visual viewport: what is left of the screen once the
// on-screen keyboard, a pinch zoom or the browser's own bars have taken theirs.
//
// `100dvh` follows the browser's bars but not the keyboard — iOS keeps the
// layout viewport at full height while the keyboard covers its lower half and
// scrolls the page instead — so a box that has to keep its bottom edge (the
// session's composer) above the keyboard sizes itself from this instead, below
// `sm` (`SPEC.md`, "Frontend", "Mobile layout"). Like `useMediaQuery`, it is for
// what a class name cannot say.

import { useSyncExternalStore } from "react";

function viewport(): VisualViewport | undefined {
  return typeof window !== "undefined"
    ? (window.visualViewport ?? undefined)
    : undefined;
}

function subscribe(onChange: () => void): () => void {
  const target = viewport();
  if (target === undefined) return () => undefined;
  // `resize` is the keyboard and the bars; `scroll` is the visual viewport
  // panning inside the layout one, which iOS does while the keyboard opens.
  target.addEventListener("resize", onChange);
  target.addEventListener("scroll", onChange);
  return () => {
    target.removeEventListener("resize", onChange);
    target.removeEventListener("scroll", onChange);
  };
}

/** Whole pixels, so a sub-pixel pan does not re-render the caller. */
function snapshot(): number | undefined {
  const target = viewport();
  return target === undefined ? undefined : Math.round(target.height);
}

/**
 * The visual viewport's height in whole CSS pixels, re-rendering when it
 * changes; `undefined` where `window.visualViewport` does not exist (jsdom, an
 * old browser, a render without a window), so the caller keeps its CSS height.
 */
export function useVisualViewportHeight(): number | undefined {
  return useSyncExternalStore(subscribe, snapshot, () => undefined);
}
