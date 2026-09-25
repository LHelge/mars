// The one confirmation for ending a session, in the session header
// (`SPEC.md`, "Frontend", Confirmations).
//
// Like the delete confirmation (`SessionDeleteConfirm`) it reads the project's
// session-branch list while it is open — the cached read the Branches tab
// shares — so that a branch with commits not on the default branch is named
// before the session ends: the tasks the session filed are not dispatched
// until those commits are merged (`ARCHITECTURE.md`, "Dispatcher"). The ref is
// the last sync's, so a live session that never synced has no row and the
// panel says nothing about commits; opening it never syncs. Until the list
// arrives, or when it cannot be read, it says nothing about commits either:
// the warning informs a decision, it never gates one.

import { useQuery } from "@tanstack/react-query";

import { ConfirmPanel } from "../components/ConfirmPanel";
import { listSessionBranches } from "../services/git";
import { queryKeys } from "../services/queryKeys";
import type { Session } from "../types";
import {
  END_CONSEQUENCES,
  sessionUnmergedWarning,
} from "./sessionUnmergedWarning";

export interface SessionEndConfirmProps {
  session: Session;
  pending: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  /** Opens the header's branch panel, where the commits can be merged. */
  onShowBranch: () => void;
}

export function SessionEndConfirm({
  session,
  pending,
  onConfirm,
  onCancel,
  onShowBranch,
}: SessionEndConfirmProps) {
  const branches = useQuery({
    queryKey: queryKeys.projects.sessionBranches(session.project_id),
    queryFn: () => listSessionBranches(session.project_id),
  });

  const warning = sessionUnmergedWarning(
    branches.data,
    session.id,
    session.branch,
    "end",
  );

  return (
    <ConfirmPanel
      message={
        <>
          End this session? {END_CONSEQUENCES}
          {warning !== null && (
            <span className="text-state-human mt-1 block font-mono text-xs">
              {warning}{" "}
              <button
                type="button"
                onClick={onShowBranch}
                className="text-console-accent hover:underline"
              >
                Open the branch panel to merge them
              </button>
            </span>
          )}
        </>
      }
      confirmLabel="End the session"
      pending={pending}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
