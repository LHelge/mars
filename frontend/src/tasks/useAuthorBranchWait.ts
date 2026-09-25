// The author-branch line's two reads (`tasks/authorBranch.ts`).
//
// The session-branch list is the cached read the Branches tab and the delete
// confirmation share, under the same key, so the board adds no git polling of
// its own: the list refreshes when a view mounts it stale, on window focus and
// when a merge invalidates the project's queries. Only a task the author rule
// could hold asks for it, so a board whose tasks were all filed by people
// never reads git at all. The session titles come from the project's cached
// session list, as the Branches tab names its rows, and are read only once
// there is a line to name a session in.

import { useQuery } from "@tanstack/react-query";

import { listSessionBranches } from "../services/git";
import { queryKeys } from "../services/queryKeys";
import { projectQueries } from "../services/queryOptions";
import type { Task } from "../types";
import {
  authorBranchMessage,
  authorBranchWait,
  waitsForAuthorBranch,
} from "./authorBranch";

type AuthorFields = Pick<
  Task,
  | "project_id"
  | "created_by_session_id"
  | "closed_at"
  | "lease_holder_session_id"
>;

/** The line's text, or `null` when the task waits for no author branch. */
export function useAuthorBranchWait(task: AuthorFields): string | null {
  const branches = useQuery({
    queryKey: queryKeys.projects.sessionBranches(task.project_id),
    queryFn: () => listSessionBranches(task.project_id),
    enabled: waitsForAuthorBranch(task),
  });

  const wait = authorBranchWait(task, branches.data);

  const sessions = useQuery({
    ...projectQueries.sessions(task.project_id),
    enabled: wait !== null,
  });

  if (wait === null) {
    return null;
  }
  const title = sessions.data?.find(
    (session) => session.id === wait.sessionId,
  )?.title;
  return authorBranchMessage(wait, title);
}
