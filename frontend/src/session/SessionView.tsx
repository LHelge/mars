// The session view: header, transcript, composer, side panel.
//
// The transcript is the page and everything else frames it — the header is one
// band above, the composer one band below, and the panel a column beside (a
// sheet over the transcript below `lg`). The whole view is one screen-height
// box that never scrolls itself: only the transcript and the open panel scroll,
// so the composer stays where the hands are and the header stays readable
// while a long run streams past.
//
// Its height is the viewport less the page's frame above and below it. From
// `sm` up that is `100dvh`; below it, where the on-screen keyboard takes half
// the screen and iOS does not shrink `dvh` for it, it is the visual viewport,
// so the composer stays above the keyboard instead of the page scrolling the
// header away (`SPEC.md`, "Frontend", "Mobile layout").

import { useCallback, useLayoutEffect, useRef, useState } from "react";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import { SM_QUERY, useMediaQuery } from "../hooks/useMediaQuery";
import { useVisualViewportHeight } from "../hooks/useVisualViewportHeight";
import type { Session } from "../types";
import { Composer } from "./Composer";
import { SessionHeader } from "./SessionHeader";
import { SessionSocketContext } from "./SessionSocketContext";
import { SidePanel } from "./SidePanel";
import { Transcript } from "./Transcript";
import type { SessionSocketApi } from "./useSessionSocket";

export interface SessionViewProps {
  session: Session;
  socket: SessionSocketApi;
}

export function SessionView({ session, socket }: SessionViewProps) {
  // A rejected message's Resend goes from the transcript to the composer
  // through the per-session UI store (`sessionUi`), so nothing about it passes
  // through this view.
  const { loadOlder, stop } = socket;
  const older = useCallback(() => {
    void loadOlder();
  }, [loadOlder]);

  const wide = useMediaQuery(SM_QUERY);
  const viewportHeight = useVisualViewportHeight();
  const box = useRef<HTMLDivElement>(null);
  /** What the page takes around the box: its top edge in the document, plus
   *  the padding below it. Measured rather than assumed, because the top bar's
   *  height is the navigation's and the safe-area insets are the device's. */
  const [frame, setFrame] = useState<number | null>(null);
  useLayoutEffect(() => {
    const element = box.current;
    if (wide || element === null) return;
    const below = (node: Element | null): number =>
      node === null ? 0 : parseFloat(getComputedStyle(node).paddingBottom) || 0;
    const top = element.getBoundingClientRect().top + window.scrollY;
    const next = Math.round(
      top + below(element.parentElement) + below(document.body),
    );
    setFrame((current) => (current === next ? current : next));
  }, [wide, viewportHeight]);
  const phoneHeight =
    !wide && viewportHeight !== undefined && frame !== null
      ? viewportHeight - frame
      : undefined;

  return (
    <SessionSocketContext.Provider value={socket}>
      {/* The console frame above this view is fixed height; the rest of the
          viewport is the session. */}
      {/* `relative`: below `sm` the header's branch sheet is positioned over
          this box. The inline height, below `sm` only, wins over the class. */}
      <div
        ref={box}
        style={phoneHeight === undefined ? undefined : { height: phoneHeight }}
        className="border-console-border bg-console-bg relative flex h-[calc(100dvh-6rem)] min-h-[16rem] flex-col overflow-hidden rounded border sm:min-h-[28rem]"
      >
        <SessionHeader session={session} status={socket.status} onStop={stop} />

        {socket.status === "offline" && (
          <div className="border-console-border border-b px-4 py-2">
            <Alert kind="error">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span>{socket.error}</span>
                <SubmitButton
                  type="button"
                  variant="ghost"
                  onClick={socket.reconnect}
                >
                  Reconnect
                </SubmitButton>
              </div>
            </Alert>
          </div>
        )}

        {/* `relative`: below `lg` the side panel is a sheet positioned inside
            this row, over the transcript and the composer. */}
        <div className="relative flex min-h-0 flex-1">
          <div className="flex min-w-0 flex-1 flex-col">
            <Transcript sessionId={session.id} loadOlder={older} />
            <Composer sessionId={session.id} />
          </div>

          <SidePanel session={session} />
        </div>
      </div>
    </SessionSocketContext.Provider>
  );
}
