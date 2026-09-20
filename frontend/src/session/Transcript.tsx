// The session transcript: a virtualised log of every message the store has
// folded (`SPEC.md`, "Frontend", "Transcript rendering"; `ARCHITECTURE.md`,
// "Event delivery").
//
// Rows have wildly different heights — a one-line state change, a long tool
// result, a whole nested subagent — so the virtualizer measures them instead of
// estimating, and nothing here caps a row's height.

import { useCallback, useLayoutEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import { EmptyState } from "../components/EmptyState";
import { Spinner } from "../components/Spinner";
import { MessageRow } from "./messages/MessageRow";
import { useSessionStore } from "./sessionStore";
import { useStickToBottom } from "./useStickToBottom";

/** How close to the top asks for the next page of history. */
const NEAR_TOP_PX = 200;
/** A first guess only; every row is measured once it is on screen. */
const ESTIMATED_ROW_PX = 72;

export interface TranscriptProps {
  sessionId: string;
  /** Fetches the page before `oldestSeq` and prepends it to the store. */
  loadOlder?: () => void;
  /** Called with the text of a rejected message the user wants to resend. */
  onResend?: (text: string) => void;
}

export function Transcript({ sessionId, loadOlder, onResend }: TranscriptProps) {
  const order = useSessionStore(sessionId, (state) => state.order);
  const hasMore = useSessionStore(sessionId, (state) => state.hasMore);
  const oldestSeq = useSessionStore(sessionId, (state) => state.oldestSeq);
  // Streaming growth has to re-pin the view, and it changes no id: the length
  // of the tail message's text is what moves while a `text_delta` arrives.
  const tailLength = useSessionStore(sessionId, (state) => {
    const last = state.order.at(-1);
    const message = last === undefined ? undefined : state.messages[last];
    return message !== undefined && "text" in message ? message.text.length : 0;
  });

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stick = useStickToBottom(scrollRef, { order, tailLength });

  const virtualizer = useVirtualizer({
    count: order.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ESTIMATED_ROW_PX,
    overscan: 8,
    getItemKey: (index) => order[index] ?? index,
  });

  // The page already asked for, so one scroll gesture near the top does not
  // fire a request per `scroll` event; a landed page moves `oldestSeq` and
  // arms the next one.
  // `undefined` is "nothing requested yet": `oldestSeq` is itself `null` before
  // the first event, and a null cursor must not read as an outstanding request.
  const requestedFor = useRef<number | null | undefined>(undefined);
  const anchor = useRef<{ height: number; top: number } | null>(null);

  const handleScroll = useCallback(() => {
    stick.onScroll();
    const element = scrollRef.current;
    if (!element || !loadOlder || !hasMore) {
      return;
    }
    if (element.scrollTop > NEAR_TOP_PX || requestedFor.current === oldestSeq) {
      return;
    }
    requestedFor.current = oldestSeq;
    anchor.current = { height: element.scrollHeight, top: element.scrollTop };
    loadOlder();
  }, [hasMore, loadOlder, oldestSeq, scrollRef, stick]);

  // A prepend grows the list above the viewport, which would otherwise drag the
  // reader backwards by exactly the height of the page that just arrived.
  useLayoutEffect(() => {
    const pending = anchor.current;
    const element = scrollRef.current;
    if (!pending || !element) {
      return;
    }
    anchor.current = null;
    // Pinned to the bottom, the tail is what the reader is watching: keep it.
    if (stick.pinned) {
      return;
    }
    element.scrollTop = element.scrollHeight - pending.height + pending.top;
  }, [oldestSeq, order, scrollRef, stick.pinned]);

  const items = virtualizer.getVirtualItems();

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      <div
        ref={scrollRef}
        onScroll={handleScroll}
        data-testid="transcript-scroll"
        className="flex-1 overflow-y-auto px-4 py-2"
      >
        {hasMore && (
          <div className="text-console-muted flex items-center justify-center gap-2 py-2 text-xs">
            <Spinner className="size-3" />
            <span>Loading earlier messages</span>
          </div>
        )}
        {order.length === 0 ? (
          <EmptyState
            title="Nothing has happened yet"
            description="Send a message to start the session."
          />
        ) : (
          <div
            style={{ height: virtualizer.getTotalSize(), position: "relative" }}
          >
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
                  id={order[item.index]}
                  onResend={onResend}
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
          className="border-console-border bg-console-raised text-console-text absolute inset-x-0 bottom-3 mx-auto w-fit rounded-full border px-3 py-1 text-xs shadow"
        >
          Jump to latest
          {stick.newCount > 0 && (
            <span className="text-console-accent pl-2">{stick.newCount}</span>
          )}
        </button>
      )}
    </div>
  );
}
