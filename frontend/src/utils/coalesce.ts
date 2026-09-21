// One run per tick, however many callers asked for it.
//
// The sibling of `debounce`, and the difference is the whole point: a debounce
// restarts its timer on every call, so a stream of events faster than the delay
// never runs the callback at all. This one starts the timer on the first call
// and ignores every call until it fires, so a burst costs exactly one run and
// a sustained stream runs once per tick rather than never.
//
// The task board's use is the stream's query invalidations (`SPEC.md`,
// "Frontend", "Board refresh ordering"): a project with agents working in it
// can emit events faster than a round trip, and each one would otherwise be a
// separate `invalidateQueries` — a scan of the whole query cache, and a
// refetch of every open task detail, per event.

export interface Coalesced {
  /** Asks for a run; the callback runs once, at the end of the current tick. */
  run: () => void;
  /** Drops a pending run. Safe to call when nothing is pending. */
  cancel: () => void;
}

/**
 * `delayMs` is 0 — the next macrotask — rather than a frame: an invalidation
 * that falls due while the tab is in the background still runs, where
 * `requestAnimationFrame` would hold every one of them until the tab is looked
 * at again.
 */
export function coalesce(callback: () => void, delayMs = 0): Coalesced {
  let timer: ReturnType<typeof setTimeout> | null = null;

  const cancel = (): void => {
    if (timer === null) return;
    clearTimeout(timer);
    timer = null;
  };

  return {
    run: () => {
      if (timer !== null) return;
      timer = setTimeout(() => {
        timer = null;
        callback();
      }, delayMs);
    },
    cancel,
  };
}
