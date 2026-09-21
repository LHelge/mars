// What a secrets mutation has to refresh besides its own list.
//
// A credential written at one scope changes which credential wins at every
// project the caller can launch in (`SPEC.md`, "Frontend": the
// `["projects", pid, "agent-credentials"]` key is "invalidated by every
// secrets mutation"), and the agent-credentials section reads several scopes
// at once, so a write at one of them is stale in the others. Both are matched
// by shape rather than enumerated: the project whose answer changed is not
// knowable from the row that was written.

import type { QueryClient } from "@tanstack/react-query";
import { queryKeys } from "../services/queryKeys";

/**
 * Invalidates every secrets list and every project's agent-credential answer.
 * Call it after any create, replace, rename, flag or delete of a secret.
 */
export function invalidateSecretQueries(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({ queryKey: queryKeys.secrets.all });
  void queryClient.invalidateQueries({
    predicate: (query) =>
      query.queryKey[0] === "projects" &&
      query.queryKey[2] === "agent-credentials",
  });
}
