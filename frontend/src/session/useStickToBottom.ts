// Auto-follow for the transcript scroller.
//
// The transcript is a log: while the user is reading the tail it should stay on
// the tail as text streams in, and the moment the user scrolls up it must stop
// moving under them. The flag is derived from `scroll` events rather than from
// an observer so it also answers correctly for a container that is not
// scrollable yet (nothing to scroll is "at the bottom").
//
// Distance from the bottom alone does not say who moved the view. A virtualised
// list renders a tall row at an estimate and corrects itself once the row is
// measured, which can leave the viewport thousands of pixels above the bottom
// without the reader touching anything; reading that as "the reader scrolled
// away" strands the transcript behind a "Jump to latest" nobody asked for. What
// separates the two is the direction the viewport itself moved: growing content
// and the virtualizer's own corrections push the bottom away, while only a
// reader (wheel, touch, keyboard or scrollbar, all of which arrive as plain
// `scroll` events) moves the viewport *upwards*. So unpinning takes both a
// distance past `NEAR_BOTTOM_PX` and a `scrollTop` lower than the last one
// observed; anything else that moved the bottom out of reach while pinned is
// followed back down.

import { useCallback, useLayoutEffect, useRef, useState } from "react";
import type { RefObject } from "react";

/** How close to the bottom still counts as following the tail. */
export const NEAR_BOTTOM_PX = 48;

export interface StickToBottomOptions {
  /** Top-level message ids in display order; identity changes on every fold. */
  order: string[];
  /** Length of the last message's text, so `text_delta` growth re-pins too. */
  tailLength: number;
  /**
   * The height of the content, when the caller knows it better than the DOM
   * does at commit time. A virtualised list renders its rows at an estimated
   * height and corrects itself once they are measured, which grows the content
   * after this effect would otherwise have run: without re-pinning on the
   * corrected height, "follow the tail" lands wherever the estimate happened to
   * put it. Any number that changes when the content grows will do.
   */
  contentHeight?: number;
  /**
   * Whether an id appended at the tail is the reader's own message. Their own
   * send re-pins the view however far up they had scrolled: they wrote it, so
   * they are following it — and the pill counting it as unread would be
   * counting what they just did. Stable across renders, or the effect below
   * runs for nothing.
   */
  isOwnAppend?: (id: string) => boolean;
}

export interface StickToBottom {
  /** Attach to the scroll container's `onScroll`. */
  onScroll: () => void;
  /** Whether the view is following the tail, as of the last render. */
  pinned: boolean;
  /**
   * The same flag, as of right now. `onScroll` updates it before React has
   * re-rendered, so a scroll handler reading `pinned` from its own closure sees
   * the value from before the gesture it is handling.
   */
  isPinned: () => boolean;
  /** Top-level messages appended since the user scrolled up. */
  newCount: number;
  /** Re-pin to the tail, which the `Jump to latest` pill calls. */
  jumpToLatest: () => void;
}

/**
 * Messages appended after `lastId`; the whole list when there was no tail yet.
 *
 * A tail id that is no longer in `order` is zero growth, not a whole list of
 * it: the one thing that removes an id is a rename in place — the optimistic
 * `client:<id>` the user's own `user_message` event turns into `e<seq>`, at the
 * same position — and reading that as "everything is new" is how the pill came
 * to say "Jump to latest 413".
 */
function appendedAfter(order: string[], lastId: string | null): number {
  if (lastId === null) {
    return order.length;
  }
  const index = order.lastIndexOf(lastId);
  return index === -1 ? 0 : order.length - 1 - index;
}

/**
 * The scroll container is passed in rather than returned, so the hook's result
 * is plain state and callbacks and the caller owns the one ref.
 */
export function useStickToBottom(
  scrollRef: RefObject<HTMLDivElement | null>,
  { order, tailLength, contentHeight = 0, isOwnAppend }: StickToBottomOptions,
): StickToBottom {
  // The flag is needed synchronously inside the layout effect, where the state
  // value would still be the one from the render that is being committed.
  const pinnedRef = useRef(true);
  const [pinned, setPinned] = useState(true);
  const [newCount, setNewCount] = useState(0);
  const lastIdRef = useRef<string | null>(null);
  // The viewport position as of the last time it was seen, to compare the next
  // one against. `null` is "never seen": a first `scroll` event has nothing to
  // be a movement relative to, so it is judged on its distance alone.
  const lastTopRef = useRef<number | null>(null);

  const scrollToBottom = useCallback(() => {
    const element = scrollRef.current;
    if (element) {
      element.scrollTop = element.scrollHeight;
      // The browser clamps that assignment to the scrollable range, and the
      // clamped value is the position the next `scroll` event is read against.
      lastTopRef.current = element.scrollTop;
    }
  }, [scrollRef]);

  const onScroll = useCallback(() => {
    const element = scrollRef.current;
    if (!element) {
      return;
    }
    const top = element.scrollTop;
    const previous = lastTopRef.current;
    lastTopRef.current = top;
    const distance = element.scrollHeight - top - element.clientHeight;
    if (distance <= NEAR_BOTTOM_PX) {
      pinnedRef.current = true;
      setPinned(true);
      setNewCount(0);
      return;
    }
    if (previous !== null && top >= previous) {
      // The bottom moved away from a viewport that stayed where it was: the
      // content grew, or the virtualizer corrected its own estimate. Follow it.
      if (pinnedRef.current) {
        scrollToBottom();
      }
      return;
    }
    pinnedRef.current = false;
    setPinned(false);
  }, [scrollRef, scrollToBottom]);

  useLayoutEffect(() => {
    // Counting from the previous tail id rather than from the previous length
    // keeps a prepended history page out of the "new messages" count.
    const appended = appendedAfter(order, lastIdRef.current);
    const tail = order[order.length - 1] ?? null;
    lastIdRef.current = tail;
    // The reader has just sent something: pin before the branch below, so
    // following their own message down is the same path as following the tail
    // — and the pill never counts what they themselves did.
    if (
      !pinnedRef.current &&
      appended > 0 &&
      tail !== null &&
      isOwnAppend?.(tail) === true
    ) {
      pinnedRef.current = true;
    }
    if (pinnedRef.current) {
      scrollToBottom();
      setNewCount(0);
      // Almost always the value it already holds, which React drops without
      // re-rendering; it is the re-pin above that makes it worth setting.
      setPinned(true);
      return;
    }
    if (appended > 0) {
      setNewCount((count) => count + appended);
    }
  }, [order, tailLength, contentHeight, isOwnAppend, scrollToBottom]);

  const jumpToLatest = useCallback(() => {
    pinnedRef.current = true;
    setPinned(true);
    setNewCount(0);
    scrollToBottom();
  }, [scrollToBottom]);

  const isPinned = useCallback(() => pinnedRef.current, []);

  return { onScroll, pinned, isPinned, newCount, jumpToLatest };
}
