// "Which credential would this launch use?", for one project and one backend
// (`SPEC.md`, "Frontend", Agent credentials).
//
// One query per project under `["projects", pid, "agent-credentials"]` — the
// key `invalidateSecretQueries` matches, so a credential written anywhere
// refreshes every open answer — and the backend picks an entry out of it
// rather than being part of the key: the endpoint answers for every backend
// at once, so a profile editor and a launch form on the same page share one
// request.
//
// The answer is per caller. Nothing here is stored on the profile and nothing
// is cached beyond the session: a teammate opening the same profile sees their
// own credential (ADR 0036).

import { useQuery } from "@tanstack/react-query";

import { queryKeys } from "../services/queryKeys";
import { getAgentCredentials } from "../services/secrets";
import type { AgentCredential } from "../types";

export interface AgentCredentialAnswer {
  /**
   * The credential that would be injected, or `null` when the answer arrived
   * and there is none. `undefined` while it is still unknown — loading, an
   * error, or a backend this response has no entry for — which is what keeps
   * the notice quiet instead of warning about something it has not read.
   */
  credential: AgentCredential | null | undefined;
  /** True until the first answer for this project has arrived. */
  isPending: boolean;
  /** The endpoint failed; the notice says nothing rather than guessing. */
  isError: boolean;
}

/**
 * The entry for `backend`, out of the project's agent-credential answer. A
 * `backend` this build's server does not report — an image on a newer
 * orchestrator, a profile from the future — leaves `credential` undefined and
 * renders nothing (`SPEC.md`, "Frontend").
 */
export function useAgentCredential(
  projectId: string,
  backend: string,
): AgentCredentialAnswer {
  const query = useQuery({
    queryKey: queryKeys.projects.agentCredentials(projectId),
    queryFn: () => getAgentCredentials(projectId),
  });

  const entry = query.data?.find((status) => status.backend === backend);

  return {
    credential: entry === undefined ? undefined : entry.credential,
    isPending: query.isPending,
    isError: query.isError,
  };
}
