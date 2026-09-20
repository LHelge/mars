// `POST /sessions/{id}/sync`: fetch the session's work clone into the project
// mirror (`SPEC.md`, "Sessions"; `ARCHITECTURE.md`, "Git model").
//
// Two places offer it — the header's action row and the Branch section, which
// is the one thing to do about a session that has never been synced — and both
// mean the same call, so the mutation lives here instead of twice.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { UseMutationResult } from "@tanstack/react-query";

import { queryKeys } from "../services/queryKeys";
import { syncSession } from "../services/sessions";
import type { SyncResult } from "../types";

export interface UseSyncSessionOptions {
  onSuccess?: (result: SyncResult) => void;
  onError?: (caught: unknown) => void;
}

/**
 * A sync moves the session's ref in the mirror, so everything the project's
 * git views hold — the session branches with their ahead/behind, the branch
 * list — is stale the moment it returns.
 */
export function useSyncSession(
  sessionId: string,
  projectId: string,
  options: UseSyncSessionOptions = {},
): UseMutationResult<SyncResult, unknown, void> {
  const queryClient = useQueryClient();
  const { onSuccess, onError } = options;

  return useMutation<SyncResult, unknown, void>({
    mutationFn: () => syncSession(sessionId),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.detail(projectId),
      });
      onSuccess?.(result);
    },
    onError: (caught: unknown) => {
      onError?.(caught);
    },
  });
}
