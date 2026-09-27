// The session transcript: a virtualised log of every message the store has
// folded (`SPEC.md`, "Frontend", "Transcript rendering"; `ARCHITECTURE.md`,
// "Event delivery").
//
// Rows have wildly different heights — a one-line state change, a long tool
// result, a whole nested subagent — so the virtualizer measures them instead of
// estimating, and nothing here caps a row's height.
//
// There is no React Compiler in this build (`ARCHITECTURE.md`, "Frontend
// architecture"), so every re-render boundary here is explicit: this component
// subscribes to `tailLength` and therefore re-renders on every `text_delta`,
// and what keeps that from costing the viewport is `MessageRow` being
// `memo()`-wrapped and what it is handed per row — the session id and the
// message id — being stable. `getItemKey` is stable per `order` for the same
// reason: a fresh identity resets the virtualizer's measurement cache, which
// would re-measure every row on every delta.

import { useCallback, useLayoutEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import { EmptyState } from "../components/EmptyState";
import { TRANSCRIPT_SCROLL } from "../utils/testIds";
import { Spinner } from "../components/Spinner";
import { MessageRow } from "./messages/MessageRow";
import { isOptimisticId, useSessionStore } from "./sessionStore";
import { SessionUiContext } from "./sessionUi";
import { useStickToBottom } from "./useStickToBottom";
import { TAP, TAP_INLINE } from "../components/fieldStyles";

/** How close to the top asks for the next page of history. */
const NEAR_TOP_PX = 200;
/** A first guess only; every row is measured once it is on screen. */
const ESTIMATED_ROW_PX = 72;

export interface TranscriptProps {
  sessionId: string;
  /** Fetches the page before `oldestSeq` and prepends it to the store. */
  loadOlder?: () => void;
}

export function Transcript({ sessionId, loadOlder }: TranscriptProps) {
  const order = useSessionStore(sessionId, (state) => state.order);
  const hasMore = useSessionStore(sessionId, (state) => state.hasMore);
  const oldestSeq = useSessionStore(sessionId, (state) => state.oldestSeq);
  const historyStatus = useSessionStore(
    sessionId,
    (state) => state.historyStatus,
  );
  const historyError = useSessionStore(
    sessionId,
    (state) => state.historyError,
  );
  // Streaming growth has to re-pin the view, and it changes no id: the length
  // of the tail message's text is what moves while a `text_delta` arrives.
  const tailLength = useSessionStore(sessionId, (state) => {
    const last = state.order.at(-1);
    const message = last === undefined ? undefined : state.messages[last];
    return message !== undefined && "text" in message ? message.text.length : 0;
  });

  const scrollRef = useRef<HTMLDivElement | null>(null);

  // Stable per `order`: the virtualizer memoises its measurements on this
  // function's identity, so an inline one would throw the measurement cache
  // away on every delta and re-measure the whole list.
  const getItemKey = useCallback(
    (index: number) => order[index] ?? index,
    [order],
  );

  // The virtualizer returns functions the hooks plugin cannot prove safe to
  // memoise. `SPEC.md` names this library for the transcript, nothing here
  // memoises what it returns, and the boundaries that matter are the explicit
  // ones above.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: order.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ESTIMATED_ROW_PX,
    overscan: 8,
    getItemKey,
  });

  // Rows are measured after they are rendered, so the list's height keeps
  // changing under a reader who is following the tail; the total size is what
  // tells the auto-follow to re-pin once the measurements land.
  const totalSize = virtualizer.getTotalSize();
  const stick = useStickToBottom(scrollRef, {
    order,
    tailLength,
    contentHeight: totalSize,
    // A message this browser added itself is the reader's own send.
    isOwnAppend: isOptimisticId,
  });

  // The page already asked for, so one scroll gesture near the top does not
  // fire a request per `scroll` event; a landed page moves `oldestSeq` and
  // arms the next one.
  // `undefined` is "nothing requested yet": `oldestSeq` is itself `null` before
  // the first event, and a null cursor must not read as an outstanding request.
  const requestedFor = useRef<number | null | undefined>(undefined);
  /**
   * Where the reader was when the page was asked for, and the cursor that was
   * current then. The cursor is what says the page has landed: until
   * `oldestSeq` moves past it, every other commit in between — and `pinned`
   * flipping as the reader scrolls up is one — must leave the anchor alone.
   */
  const anchor = useRef<{
    height: number;
    top: number;
    pinned: boolean;
    requestedFor: number | null;
  } | null>(null);

  // `stick` is a fresh object every render, so the handlers below depend on the
  // two stable callbacks instead of on it.
  const { onScroll: stickOnScroll, isPinned } = stick;

  /**
   * Asks for the page before `oldestSeq`, remembering where the reader was.
   *
   * Three things stop a request: no page to ask for, one already in flight —
   * a scroll gesture and the fill effect below can want the same page in the
   * same commit — and the cursor guard, which keeps one gesture from firing a
   * request per `scroll` event and re-arms when a landed page moves
   * `oldestSeq`. A failed request leaves the status at `error` and stops the
   * automatic paths rather than retrying into the same failure; `force` is the
   * reader pressing Retry, which is the one thing that asks again.
   */
  const load = useCallback(
    (force: boolean) => {
      const element = scrollRef.current;
      if (!element || !loadOlder || !hasMore || historyStatus === "loading") {
        return;
      }
      if (
        !force &&
        (historyStatus === "error" || requestedFor.current === oldestSeq)
      ) {
        return;
      }
      requestedFor.current = oldestSeq;
      anchor.current = {
        height: element.scrollHeight,
        top: element.scrollTop,
        // The live flag: `stick.pinned` is still the value from before this
        // very gesture (`useStickToBottom`, `isPinned`).
        pinned: isPinned(),
        requestedFor: oldestSeq,
      };
      loadOlder();
    },
    [hasMore, historyStatus, isPinned, loadOlder, oldestSeq, scrollRef],
  );

  const handleScroll = useCallback(() => {
    stickOnScroll();
    const element = scrollRef.current;
    if (!element || element.scrollTop > NEAR_TOP_PX) {
      return;
    }
    load(false);
  }, [load, scrollRef, stickOnScroll]);

  const retry = useCallback(() => {
    load(true);
  }, [load]);

  // A prepend grows the list above the viewport, which would otherwise drag the
  // reader backwards by exactly the height of the page that just arrived.
  useLayoutEffect(() => {
    const pending = anchor.current;
    const element = scrollRef.current;
    if (!pending || !element || pending.requestedFor === oldestSeq) {
      return;
    }
    anchor.current = null;
    // Pinned to the bottom, the tail is what the reader is watching: keep it.
    if (pending.pinned) {
      return;
    }
    element.scrollTop = element.scrollHeight - pending.height + pending.top;
  }, [oldestSeq, order, scrollRef]);

  // A failed page moved nothing, so the anchor it took is stale and the cursor
  // guard would otherwise refuse that page for the rest of the mount. Both go;
  // what holds the automatic paths back now is the `error` status, which Retry
  // is the only way past.
  useLayoutEffect(() => {
    if (historyStatus !== "error") {
      return;
    }
    anchor.current = null;
    requestedFor.current = undefined;
  }, [historyStatus]);

  // A page of 200 events can fold into one or two rows — `text_delta` events
  // are tiny — so a transcript that does not overflow its viewport never
  // produces a `scroll` event, and scrolling would be the only way to ask for
  // the rest of the history. One page at a time, until the content overflows
  // or there is nothing older left.
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element || element.scrollHeight > element.clientHeight + NEAR_TOP_PX) {
      return;
    }
    load(false);
  }, [load, order, scrollRef, totalSize]);

  const items = virtualizer.getVirtualItems();

  return (
    // Every row below this point is a row of this session: what the reader
    // opens is keyed by the session and the message id, and a tool renderer
    // deep in a row reaches that key without the id being threaded through the
    // registry's props (`sessionUi`).
    <SessionUiContext.Provider value={sessionId}>
      <div className="relative flex h-full min-h-0 flex-col">
        {/* Outside the scroller on purpose: anything above the virtual container
            inside it would shift every row's offset away from what the
            virtualizer positions rows at. */}
        {historyStatus === "loading" && (
          <div className="text-console-muted flex shrink-0 items-center justify-center gap-2 py-2 text-xs">
            <Spinner className="size-3" />
            <span>Loading earlier messages</span>
          </div>
        )}
        {historyStatus === "error" && (
          <div className="text-state-failed flex shrink-0 items-center justify-center gap-2 py-2 text-xs">
            <span>
              Could not load earlier messages
              {historyError === null ? "" : `: ${historyError}`}
            </span>
            <button
              type="button"
              onClick={retry}
              className={`border-console-border text-console-text rounded border px-2 py-0.5 ${TAP_INLINE}`}
            >
              Retry
            </button>
          </div>
        )}
        <div
          ref={scrollRef}
          onScroll={handleScroll}
          data-testid={TRANSCRIPT_SCROLL}
          className="min-h-0 flex-1 overflow-y-auto px-4 py-2"
        >
          {order.length === 0 ? (
            <EmptyState
              title="Nothing has happened yet"
              description="Send a message to start the session."
            />
          ) : (
            <div style={{ height: totalSize, position: "relative" }}>
              {items.map((item) => (
                <div
                  key={item.key}
                  data-index={item.index}
                  ref={virtualizer.measureElement}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${item.start}px)`,
                  }}
                >
                  <MessageRow
                    sessionId={sessionId}
                    id={order[item.index] ?? ""}
                  />
                </div>
              ))}
            </div>
          )}
        </div>
        {!stick.pinned && (
          <button
            type="button"
            onClick={stick.jumpToLatest}
            className={`border-console-border bg-console-raised text-console-text absolute inset-x-0 bottom-3 mx-auto w-fit rounded-full border px-3 py-1 text-xs shadow ${TAP}`}
          >
            Jump to latest
            {stick.newCount > 0 && (
              <span className="text-console-accent pl-2">{stick.newCount}</span>
            )}
          </button>
        )}
      </div>
    </SessionUiContext.Provider>
  );
}
