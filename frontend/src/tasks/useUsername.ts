// Who a user id is, for the places a task names one: the assignee on a card or
// in the drawer, and the author of a comment.
//
// `staleTime: Infinity` because a username barely changes and one board can
// carry the same one on fifty cards; until the read lands, the id's first
// eight characters say who it is well enough to recognise. A user the viewer
// may not read is a 403, which the shared retry policy already never retries
// (`queryClient.ts`).

import { useQuery } from "@tanstack/react-query";

import { queryKeys } from "../services/queryKeys";
import { getUser } from "../services/users";

/** The username, the shortened id while it loads, or `null` for no user. */
export function useUsername(id: string | null): string | null {
  const user = useQuery({
    queryKey: queryKeys.users.detail(id ?? ""),
    queryFn: () => getUser(id ?? ""),
    enabled: id !== null,
    staleTime: Infinity,
  });

  if (id === null) {
    return null;
  }
  return user.data?.username ?? id.slice(0, 8);
}
