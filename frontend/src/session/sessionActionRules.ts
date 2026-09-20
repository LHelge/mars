// Which lifecycle actions a session offers, as one pure rule per verb.
//
// The contract is the server's: `SPEC.md`, "Sessions" says which states each
// endpoint accepts, and `ARCHITECTURE.md`, "Session lifecycle" says why. The
// buttons must offer exactly those, so the rules live here rather than inside
// `SessionActions.tsx` — a module that renders a component exports nothing else
// (`react-refresh/only-export-components`) — and a unit test holds them against
// the state table.
//
//   Stop    running                 SIGINT now, SIGTERM after the grace period
//   End     creating/running/parked stop or cancel the launch, fetch back, `done`
//   Sync    running/parked/done     fetch the session branch into the mirror
//   Retry   failed, conversational  relaunch, optionally with a new message
//   Delete  done/failed             remove the session and leave the page
//
// `End` during `creating` is the user who launched by mistake: the orchestrator
// cancels the launch, removes any container it had got as far as creating and
// closes the session `done`, so the button does not wait for a container the
// user never wanted (`ARCHITECTURE.md`, "Session lifecycle", "A session ended
// while it is creating").

import type { Session } from "../types";

/** What the buttons of one session are, all five decided together. */
export interface SessionActionAvailability {
  stop: boolean;
  end: boolean;
  sync: boolean;
  retry: boolean;
  delete: boolean;
}

/**
 * The five rules for one session, from its state and kind alone.
 *
 * Nothing else is read: a refusal the state does not predict — a session that
 * ended between the render and the press — is the server's sentence, shown as
 * it comes back.
 */
export function sessionActions(session: Session): SessionActionAvailability {
  const state = session.state;

  return {
    stop: state === "running",
    end: state === "creating" || state === "running" || state === "parked",
    sync: state === "running" || state === "parked" || state === "done",
    retry: state === "failed" && session.kind === "conversational",
    delete: state === "done" || state === "failed",
  };
}
