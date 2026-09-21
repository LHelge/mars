// The one read of `GET /projects/{id}` the project page and every one of its
// tabs share (`SPEC.md`, "Projects"). Tab panels take the `Project` as a prop
// instead of calling this again, so a tab switch never re-fetches.
//
// The query itself — key, request and the poll a `cloning` project needs — is
// `projectQueries.detail` in `services/queryOptions.ts`, so the views that
// cannot take the project as a prop (the session view, the task drawer) read
// it exactly as this page does rather than with their own spelling of the same
// key.

import { useQuery } from "@tanstack/react-query";
import { projectQueries } from "../../services/queryOptions";

export function useProject(id: string) {
  return useQuery(projectQueries.detail(id));
}
