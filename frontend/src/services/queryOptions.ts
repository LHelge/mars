// The reads more than one view makes, spelled once (`SPEC.md`, "Projects",
// "Sessions"). A key belongs in `queryKeys.ts`; the query that answers it —
// the key, its `queryFn` and whatever polling is part of the answer rather
// than of one caller — belongs here, beside it.
//
// The reason is not brevity. Two views reading the same key with different
// options are two different reads of one cache entry: TanStack keeps one entry
// but one observer per call site, so a poll spelled in one of them runs only
// while that one is mounted, and an option the other forgot silently changes
// what the first sees. The project detail is the example: whether a cloning
// project is re-read is part of the answer, not of one caller.
//
// A call site still adds what is genuinely its own — `enabled`, a placeholder,
// a poll that belongs to that screen — by spreading the factory and overriding
// it, which reads as the exception it is.
//
// The retry policy is never among them: it lives in `queryClient.ts` alone
// (`SPEC.md`, "Frontend", Read failures).

import { queryOptions } from "@tanstack/react-query";

import { listProfiles } from "./profiles";
import { getProject, listBranches } from "./projects";
import { queryKeys } from "./queryKeys";
import { listProjectSessions } from "./sessions";
import type { SessionState } from "../types";

/**
 * How often a project that is still cloning is re-read. A `cloning` project is
 * being set up in the background (`ARCHITECTURE.md`, "Git model", Project
 * clone), so the read polls while it is and stops the moment the status
 * settles on `ready` or `error`.
 */
const CLONING_POLL_MS = 3000;

export const projectQueries = {
  /** `GET /projects/{id}`, as the project page, the session view and the task drawer read it. */
  detail: (id: string) =>
    queryOptions({
      queryKey: queryKeys.projects.detail(id),
      queryFn: () => getProject(id),
      refetchInterval: (query) =>
        query.state.data?.status === "cloning" ? CLONING_POLL_MS : false,
    }),

  /** `GET /projects/{id}/branches`: the mirror's refs, for every base and target picker. */
  branches: (id: string) =>
    queryOptions({
      queryKey: queryKeys.projects.branches(id),
      queryFn: () => listBranches(id),
    }),

  /** `GET /projects/{id}/profiles`: what the launch forms and the profiles tab offer. */
  profiles: (id: string) =>
    queryOptions({
      queryKey: queryKeys.projects.profiles(id),
      queryFn: () => listProfiles(id),
    }),

  /**
   * `GET /projects/{id}/sessions`, optionally filtered. A state filter extends
   * the key with a further element, so invalidating the unfiltered key covers
   * every filter at once — which is what a launch does.
   */
  sessions: (id: string, state?: SessionState) =>
    queryOptions({
      queryKey:
        state === undefined
          ? queryKeys.projects.sessions(id)
          : ([...queryKeys.projects.sessions(id), state] as const),
      queryFn: () => listProjectSessions(id, state),
    }),
};
