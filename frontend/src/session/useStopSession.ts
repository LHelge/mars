// One `Stop` behaviour, shared by the two places that offer it: the composer
// beside the transcript and the session header's actions.
//
// A stop is a request, not a transaction (`ARCHITECTURE.md`, "Stop
// semantics"). The button therefore says it has asked — and refuses to ask
// twice — until one of two things happens: the session leaves `running`, which
// is the answer, or the timeout passes without one, after which the operator
// may ask again rather than being left with a dead control.
//
// The session store is the external system this hook watches, so the answer is
// taken from the same `session` frame both buttons already render from, not
// from the outcome of the call that sent the request.

import { useEffect, useRef, useState } from "react";

import { getSessionStore } from "./sessionStore";

/**
 * A stop is a request, not a transaction: if no state change arrives, the
 * button becomes usable again rather than staying stuck.
 */
export const STOP_TIMEOUT_MS = 30_000;

export interface StopControl {
  /** The request is outstanding: the button is disabled and says so. */
  stopping: boolean;
  /** What the `Stop` button does. */
  requestStop: () => void;
  /** `Stopping…` or `Stop`, so both buttons carry the same word. */
  label: string;
}

/**
 * `stop` is the socket's stop, which falls back to
 * `POST /sessions/{id}/stop` (`SPEC.md`, "WebSocket: session stream").
 */
export function useStopSession(
  sessionId: string,
  stop: () => void,
): StopControl {
  const [stopping, setStopping] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return getSessionStore(sessionId).subscribe((next) => {
      if (next.session?.state !== "running") setStopping(false);
    });
  }, [sessionId]);

  useEffect(() => {
    return () => {
      if (timer.current !== null) clearTimeout(timer.current);
    };
  }, []);

  const requestStop = (): void => {
    setStopping(true);
    stop();
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      setStopping(false);
    }, STOP_TIMEOUT_MS);
  };

  return { stopping, requestStop, label: stopping ? "Stopping…" : "Stop" };
}
