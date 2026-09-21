// `true` only once something has been true for a while, and `false` the moment
// it stops.
//
// For a progress marker that must not appear for work that is over before it
// can be read. The task board's "Refreshing" is one: a refresh is two REST
// reads and usually finishes in a fraction of a second, and the marker lives
// in a `role="status"`, so flipping it on every task event means a screen
// reader announcing it for as long as an agent keeps working.

import { useEffect, useState } from "react";

/** How long something has to be true before it is worth saying. */
export const DELAYED_FLAG_MS = 600;

export function useDelayedFlag(active: boolean, delayMs = DELAYED_FLAG_MS) {
  const [settled, setSettled] = useState(false);

  // Nothing is set in the effect's body: the timer sets the flag, and the
  // cleanup that runs when `active` goes away — or when the delay changes —
  // puts it back, so the next spell of work waits its own delay out.
  useEffect(() => {
    if (!active) return;
    const timer = setTimeout(() => {
      setSettled(true);
    }, delayMs);
    return () => {
      clearTimeout(timer);
      setSettled(false);
    };
  }, [active, delayMs]);

  return active && settled;
}
