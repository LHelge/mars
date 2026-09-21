// The "Users" half of `/admin` (`SPEC.md`, "User-facing features", Users
// (admin): "List and delete users, toggle admin, send and revoke invites.").
//
// This half reads the list; the actions belong to `UserRow`, one observer and
// one answer line per row, so a slow delete keeps its own row busy however
// many other rows are pressed meanwhile. The checkbox is controlled by the
// query data and never by local state, so a rejected toggle simply stays where
// it was.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { useAuth } from "../../hooks";
import { queryKeys } from "../../services/queryKeys";
import { listUsers } from "../../services/users";
import { errorMessage } from "../../services/errorMessage";
import { EmptyState } from "../EmptyState";
import { LoadingState } from "../LoadingState";
import { QueryErrorAlert } from "../QueryErrorAlert";
import { SectionHeader } from "../SectionHeader";
import { HEAD, SCROLLER, TABLE, THEAD } from "./tableStyles";
import { UserRow } from "./UserRow";

export function UsersTable() {
  const { user: me } = useAuth();

  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    placeholderData: keepPreviousData,
  });

  const rows = useMemo(
    () =>
      [...(users.data ?? [])].sort((a, b) =>
        a.username.localeCompare(b.username),
      ),
    [users.data],
  );

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Users"
        description="Everyone with an account. At least one administrator must remain."
      />

      {users.isError && (
        <QueryErrorAlert
          query={users}
          message={`Could not load users. ${errorMessage(users.error)}`}
        />
      )}

      {/* "No users" is a fact only a successful read has (`SPEC.md`,
          "Frontend", Read failures). */}
      {users.isPending ? (
        <LoadingState />
      ) : rows.length === 0 ? (
        users.isSuccess && (
          <EmptyState
            title="No users"
            description="Invite someone below to get started."
          />
        )
      ) : (
        <div className={SCROLLER}>
          <table className={TABLE}>
            <thead className={THEAD}>
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  Username
                </th>
                <th scope="col" className={HEAD}>
                  Email
                </th>
                <th scope="col" className={HEAD}>
                  Admin
                </th>
                <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
                  Must change password
                </th>
                <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
                  Email notices
                </th>
                <th scope="col" className={`${HEAD} hidden md:table-cell`}>
                  Created
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((user) => (
                <UserRow
                  key={user.id}
                  user={user}
                  isSelf={user.id === me?.id}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
