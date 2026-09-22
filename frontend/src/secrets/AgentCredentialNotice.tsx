// One line saying what a session will authenticate with, wherever a session is
// about to be launched or configured (`SPEC.md`, "Frontend", Agent
// credentials): the profile editor beside the secrets field, and both launch
// forms beside their button.
//
// It is a statement, not a control. The answer is per caller and per project
// and nothing about it is stored on the profile, so a teammate opening the
// same profile reads their own credential (ADR 0036).
//
// Quiet by design. While the answer is on its way, or when the endpoint failed,
// or when the profile names a backend this server does not report, the line
// holds its height and says nothing: a launch form must not flicker a warning
// at somebody who does have a credential, and must never block on this read —
// the server launches either way and reports a `launch_warning` if it has to.

import { Link } from "react-router";

import { Icon, ICON_CLASS } from "../components/icons";

import type { AgentCredential } from "../types";
import { labelForCredential } from "./agentCredentials";
import { useAgentCredential } from "./useAgentCredential";

export interface AgentCredentialNoticeProps {
  projectId: string;
  /** The `backend` of the profile at hand; an unknown one renders nothing. */
  backend: string;
}

/** The warning wording of `SPEC.md`, "Frontend", verbatim. */
const MISSING = "No agent credential: sessions of this profile will fail to authenticate";

export function AgentCredentialNotice({
  projectId,
  backend,
}: AgentCredentialNoticeProps) {
  const { credential } = useAgentCredential(projectId, backend);

  if (credential === undefined) {
    // Loading, failed, or a backend with no entry: one blank line of the same
    // height, so the form below it does not move when the answer arrives.
    return <p className="text-xs" aria-hidden="true">&nbsp;</p>;
  }

  if (credential === null) {
    return (
      <p className="text-state-parked flex items-start gap-1.5 text-xs">
        <Icon.warning aria-hidden="true" className={`mt-px ${ICON_CLASS}`} />
        <span>
          {MISSING}.{" "}
          <Link to="/secrets" className="underline">
            Add one
          </Link>
        </span>
      </p>
    );
  }

  return (
    <p className="text-console-muted text-xs">
      {sentenceFor(credential)}
    </p>
  );
}

/**
 * `Authenticates with your …`, `… the project's …`, `… the shared …` — whose
 * credential won, in the words of `SPEC.md`, "Frontend". The credential itself
 * is named by its label and never by the environment variable
 * (`agentCredentials.ts` is the only module that spells one).
 */
function sentenceFor(credential: AgentCredential): string {
  const label = labelForCredential(credential.name);
  switch (credential.scope) {
    case "user":
      return `Authenticates with your ${label}`;
    case "project":
      return `Authenticates with the project's ${label}`;
    case "global":
      return `Authenticates with the shared ${label}`;
  }
}
