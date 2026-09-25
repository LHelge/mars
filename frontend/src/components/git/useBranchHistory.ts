// The first-parent history of one integration head, page by page
// (`SPEC.md`, "Git": `GET /projects/{pid}/git/history`).
//
// One infinite query per head under `["projects", id, "git", "history"]`, so
// the invalidation every git write already makes of the whole project — a
// merge, a rebase, a push (`GitActionsPanel`, `refresh`) — refreshes it too,
// and a revert invalidates every head's at once. A refetch re-reads each page
// from the head down, so the pages stay contiguous: whatever is loaded is an
// unbroken run of the first-parent line starting at the head.

import { useInfiniteQuery } from "@tanstack/react-query";

import { listHistory } from "../../services/git";
import { queryKeys } from "../../services/queryKeys";
import { HISTORY_PAGE_SIZE, nextHistoryCursor } from "./history";

export function useBranchHistory(projectId: string, branch: string | null) {
  return useInfiniteQuery({
    queryKey: queryKeys.projects.history(projectId, branch ?? ""),
    queryFn: ({ pageParam, signal }) =>
      listHistory(
        projectId,
        {
          branch: branch ?? undefined,
          before: pageParam,
          limit: HISTORY_PAGE_SIZE,
        },
        signal,
      ),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => nextHistoryCursor(last),
    enabled: branch !== null,
  });
}
