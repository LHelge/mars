// The one socket of the session page, handed to everything under it.
//
// `SessionPage` mounts `useSessionSocket` once and hands the API to
// `SessionView`, which publishes it here, so a side panel registered from
// another module — the Changes panel, the terminal — reaches the same
// connection without being threaded down as a prop. No component lives in this module: a file that renders one may export
// nothing else (`react-refresh/only-export-components`).

import { createContext, useContext } from "react";

import type { SessionSocketApi } from "./useSessionSocket";

export const SessionSocketContext = createContext<SessionSocketApi | null>(null);

/**
 * The session socket of the enclosing page. Throws outside the provider, which
 * is a wiring mistake rather than a state a component should render for.
 */
export function useSessionSocketApi(): SessionSocketApi {
  const socket = useContext(SessionSocketContext);
  if (socket === null) {
    throw new Error("useSessionSocketApi used outside a SessionSocketContext");
  }
  return socket;
}
