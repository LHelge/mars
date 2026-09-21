// The session view: header, transcript, composer, side panel.
//
// The transcript is the page and everything else frames it — the header is one
// band above, the composer one band below, and the panel a column beside. The
// whole view is one screen-height box that never scrolls itself: only the
// transcript and the open panel scroll, so the composer stays where the hands
// are and the header stays readable while a long run streams past.

import { useCallback, useState } from "react";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
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
  // The transcript hands a rejected message back to be edited and resent; the
  // composer takes it and says so, which clears it again.
  const [resend, setResend] = useState<string | undefined>(undefined);

  const { loadOlder, stop } = socket;
  const older = useCallback(() => {
    void loadOlder();
  }, [loadOlder]);
  const consumed = useCallback(() => {
    setResend(undefined);
  }, []);

  return (
    <SessionSocketContext.Provider value={socket}>
      {/* The console frame above this view is fixed height; the rest of the
          viewport is the session. */}
      <div className="border-console-border bg-console-bg flex h-[calc(100dvh-6rem)] min-h-[28rem] flex-col overflow-hidden rounded border">
        <SessionHeader
          session={session}
          status={socket.status}
          onStop={stop}
        />

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

        <div className="flex min-h-0 flex-1">
          <div className="flex min-w-0 flex-1 flex-col">
            <Transcript
              sessionId={session.id}
              loadOlder={older}
              onResend={setResend}
            />
            <Composer
              sessionId={session.id}
              socket={socket}
              initialText={resend}
              onConsumedInitialText={consumed}
            />
          </div>

          <SidePanel session={session} />
        </div>
      </div>
    </SessionSocketContext.Provider>
  );
}
