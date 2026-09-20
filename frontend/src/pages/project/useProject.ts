// The one read of `GET /projects/{id}` the project page and every one of its
// tabs share (`SPEC.md`, "Projects"). Tab panels take the `Project` as a prop
// instead of calling this again, so a tab switch never re-fetches.
//
// A `cloning` project is still being set up in the background
// (`ARCHITECTURE.md`, "Git model", Project clone), so the query polls while it
// is and stops the moment the status settles on `ready` or `error`.

import { useQuery } from "@tanstack/react-query";
import { getProject } from "../../services/projects";
import { queryKeys } from "../../services/queryKeys";
import type { Project } from "../../types";

/** How often a project that is still cloning is re-read. */
const CLONING_POLL_MS = 3000;

export function useProject(id: string) {
  return useQuery<Project>({
    queryKey: queryKeys.projects.detail(id),
    queryFn: () => getProject(id),
    // A 404 is an answer, not a transient failure: the page renders the
    // not-found state rather than retrying three times first.
    retry: false,
    refetchInterval: (query) =>
      query.state.data?.status === "cloning" ? CLONING_POLL_MS : false,
  });
}
