// The lifecycle state of one session (`ARCHITECTURE.md`, "Session lifecycle"),
// as the sessions list and the session header both show it.
//
// The colours are `StatusBadge`'s — this is that badge, not a second one — with
// the two things a session list needs on top of it: `creating` pulses, because
// the launch sequence is work in progress and the row will change on its own,
// and a `failed` session carries the orchestrator's `error` as the row's
// tooltip, so a failure can be read without opening the session.

import { StatusBadge } from "./StatusBadge";
import type { SessionState } from "../types";

export interface SessionStatePillProps {
  state: SessionState;
  /** The session's `error`; only shown for `failed`. */
  error?: string | null;
}

export function SessionStatePill({ state, error }: SessionStatePillProps) {
  const title =
    state === "failed" && error !== null && error !== undefined && error !== ""
      ? error
      : undefined;

  return (
    <span
      title={title}
      className={`inline-flex ${state === "creating" ? "animate-pulse" : ""}`}
    >
      <StatusBadge state={state} />
      {title !== undefined && <span className="sr-only">: {title}</span>}
    </span>
  );
}
