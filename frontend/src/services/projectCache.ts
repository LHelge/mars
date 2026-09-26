// What a project-level write does to the query cache, spelled once for every
// view that makes one (`SPEC.md`, "Projects").
//
// Two writes answer the project itself — `POST /projects/{id}/fetch` and
// `POST /projects/{id}/retry-clone` — and one takes it away. Both the projects
// table and the project page offer that pair and share this module, so a retry
// from the list and a retry from the page leave the cache in the same state.
//
// The keys matter more than they look. `queryKeys.projects.detail(id)` is
// `["projects", id]`, the prefix every read *of that project* extends —
// branches, sessions, profiles, git — while `queryKeys.projects.list()` is the
// table's own entry. Invalidating `queryKeys.projects.all` would hit both, and
// after a delete that means every read of a project that no longer exists
// refetches and 404s underneath the page that is still on screen.

import type { QueryClient } from "@tanstack/react-query";

import { queryKeys } from "./queryKeys";
import type { Project } from "../types";

/**
 * A write that answered with the project: take that answer as the new truth
 * for the detail entry, and re-read the table, whose row for it has changed.
 */
export function adoptProject(queryClient: QueryClient, updated: Project): void {
  queryClient.setQueryData(queryKeys.projects.detail(updated.id), updated);
  void queryClient.invalidateQueries({ queryKey: queryKeys.projects.list() });
}

/**
 * The project is gone (`DELETE /projects/{id}`): drop everything read under it
 * rather than invalidating it, because an invalidation of a live observer is a
 * refetch and there is nothing left to answer it — the 404 would race the
 * navigation away and put `NotFoundPage` on screen on the way out. Only the
 * table is re-read.
 */
export function forgetProject(queryClient: QueryClient, id: string): void {
  queryClient.removeQueries({ queryKey: queryKeys.projects.detail(id) });
  void queryClient.invalidateQueries({ queryKey: queryKeys.projects.list() });
}
