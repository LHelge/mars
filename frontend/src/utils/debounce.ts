// Trailing-edge debounce, with a cancel an unmount can call.
//
// The terminal panel refits on every `ResizeObserver` callback while a drag is
// in flight and must send exactly one `terminal_resize` when it settles, so the
// timer is restarted on each call and the effect's cleanup cancels it.

export interface Debounced {
  /** Restarts the timer; the callback runs once, `delayMs` after the last call. */
  run: () => void;
  /** Drops a pending run. Safe to call when nothing is pending. */
  cancel: () => void;
}

export function debounce(callback: () => void, delayMs: number): Debounced {
  let timer: ReturnType<typeof setTimeout> | null = null;

  const cancel = (): void => {
    if (timer === null) return;
    clearTimeout(timer);
    timer = null;
  };

  return {
    run: () => {
      cancel();
      timer = setTimeout(() => {
        timer = null;
        callback();
      }, delayMs);
    },
    cancel,
  };
}
