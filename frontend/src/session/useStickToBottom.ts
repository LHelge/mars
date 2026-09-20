// Auto-follow for the transcript scroller.
//
// The transcript is a log: while the user is reading the tail it should stay on
// the tail as text streams in, and the moment the user scrolls up it must stop
// moving under them. The flag is derived from `scroll` events rather than from
// an observer so it also answers correctly for a container that is not
// scrollable yet (nothing to scroll is "at the bottom").

import { useCallback, useLayoutEffect, useRef, useState } from "react";
import type { RefObject } from "react";


/** How close to the bottom still counts as following the tail. */
export const NEAR_BOTTOM_PX = 48;

export interface StickToBottomOptions {
  /** Top-level message ids in display order; identity changes on every fold. */
  order: string[];
  /** Length of the last message's text, so `text_delta` growth re-pins too. */
  tailLength: number;
}

export interface StickToBottom {
  /** Attach to the scroll container's `onScroll`. */
  onScroll: () => void;
  /** Whether the view is following the tail. */
  pinned: boolean;
  /** Top-level messages appended since the user scrolled up. */
  newCount: number;
  /** Re-pin to the tail, which the `Jump to latest` pill calls. */
  jumpToLatest: () => void;
}

/** Messages appended after `lastId`; the whole list when it is gone. */
function appendedAfter(order: string[], lastId: string | null): number {
  if (lastId === null) {
    return order.length;
  }
  const index = order.lastIndexOf(lastId);
  return index === -1 ? order.length : order.length - 1 - index;
}

/**
 * The scroll container is passed in rather than returned, so the hook's result
 * is plain state and callbacks and the caller owns the one ref.
 */
export function useStickToBottom(
  scrollRef: RefObject<HTMLDivElement | null>,
  { order, tailLength }: StickToBottomOptions,
): StickToBottom {
  // The flag is needed synchronously inside the layout effect, where the state
  // value would still be the one from the render that is being committed.
  const pinnedRef = useRef(true);
  const [pinned, setPinned] = useState(true);
  const [newCount, setNewCount] = useState(0);
  const lastIdRef = useRef<string | null>(null);

  const scrollToBottom = useCallback(() => {
    const element = scrollRef.current;
    if (element) {
      element.scrollTop = element.scrollHeight;
    }
  }, [scrollRef]);

  const onScroll = useCallback(() => {
    const element = scrollRef.current;
    if (!element) {
      return;
    }
    const distance =
      element.scrollHeight - element.scrollTop - element.clientHeight;
    const near = distance <= NEAR_BOTTOM_PX;
    pinnedRef.current = near;
    setPinned(near);
    if (near) {
      setNewCount(0);
    }
  }, [scrollRef]);

  useLayoutEffect(() => {
    // Counting from the previous tail id rather than from the previous length
    // keeps a prepended history page out of the "new messages" count.
    const appended = appendedAfter(order, lastIdRef.current);
    lastIdRef.current = order.length === 0 ? null : order[order.length - 1];
    if (pinnedRef.current) {
      scrollToBottom();
      setNewCount(0);
      return;
    }
    if (appended > 0) {
      setNewCount((count) => count + appended);
    }
  }, [order, tailLength, scrollToBottom]);

  const jumpToLatest = useCallback(() => {
    pinnedRef.current = true;
    setPinned(true);
    setNewCount(0);
    scrollToBottom();
  }, [scrollToBottom]);

  return { onScroll, pinned, newCount, jumpToLatest };
}
