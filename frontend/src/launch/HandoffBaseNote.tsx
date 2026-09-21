// The base a launch will start from, said the same way in both launch forms
// (`SPEC.md`, "Frontend", "Hand-off controls": "open in session" and "run
// once" default to the hand-off commit and visibly disclose any base
// override; `ARCHITECTURE.md`, "Launching a session for a task": an override
// confers no approval).
//
// Two sentences, and which one is shown is the whole point. Left alone, the
// launch starts where the server would start it, and saying which commit that
// is — the reviewed one — is what makes a hand-off reviewable at all. Choosing
// a base instead is a decision a reviewer has to be able to see, so it is
// stated in the warning colour rather than implied by a filled-in field.

import { Alert } from "../components/Alert";
import type { Handoff } from "../types";
import { shortSha } from "../utils/format";

export interface HandoffBaseNoteProps {
  /** The task's current hand-off, or `null` for a task without one. */
  handoff: Handoff | null;
  /** The project's default branch, which is where a launch without one starts. */
  defaultBranch: string | null;
  /** The base the user chose, or `""` for the server's own default. */
  override: string;
}

export function HandoffBaseNote({
  handoff,
  defaultBranch,
  override,
}: HandoffBaseNoteProps) {
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-console-text text-sm">
        {handoff === null ? (
          <>Base: {defaultBranch ?? "the project default"}</>
        ) : (
          <>
            Base: hand-off{" "}
            <span className="font-mono" title={handoff.commit}>
              {shortSha(handoff.commit)}
            </span>{" "}
            from {handoff.source_branch} ({handoff.review_status})
          </>
        )}
      </p>

      {override !== "" && (
        <Alert kind="warning">
          {handoff === null
            ? `Base overridden: the session starts from ${override}, not the project default.`
            : "Base overridden: the session will not start from the hand-off commit and this grants no review approval"}
        </Alert>
      )}
    </div>
  );
}
