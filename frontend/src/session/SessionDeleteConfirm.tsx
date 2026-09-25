// The one confirmation for deleting a session, on the sessions tab's row and
// in the session header alike (`SPEC.md`, "Frontend", Confirmations).
//
// It reads the project's session-branch list while it is open — the cached
// read the Branches tab shares — so that a branch with commits not on the
// default branch is named before its ref goes with the session (ADR 0049).
// Until that list arrives, or when it cannot be read, the panel says what a
// delete always removes and nothing about commits: the warning informs a
// decision, it never gates one.

import { useQuery } from "@tanstack/react-query";

import { ConfirmPanel } from "../components/ConfirmPanel";
import { listSessionBranches } from "../services/git";
import { queryKeys } from "../services/queryKeys";
import type { Session } from "../types";
import {
  DELETE_CONSEQUENCES,
  sessionUnmergedWarning,
} from "./sessionUnmergedWarning";

export interface SessionDeleteConfirmProps {
  session: Session;
  /** How the question names the session: `this session`, or its title. */
  label: string;
  confirmLabel: string;
  pending: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function SessionDeleteConfirm({
  session,
  label,
  confirmLabel,
  pending,
  onConfirm,
  onCancel,
}: SessionDeleteConfirmProps) {
  const branches = useQuery({
    queryKey: queryKeys.projects.sessionBranches(session.project_id),
    queryFn: () => listSessionBranches(session.project_id),
  });

  const warning = sessionUnmergedWarning(
    branches.data,
    session.id,
    session.branch,
    "delete",
  );

  return (
    <ConfirmPanel
      message={
        <>
          Delete {label}? {DELETE_CONSEQUENCES}
          {warning !== null && (
            <span className="text-state-human mt-1 block font-mono text-xs">
              {warning}
            </span>
          )}
        </>
      }
      confirmLabel={confirmLabel}
      pending={pending}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
