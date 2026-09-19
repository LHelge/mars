// The "Users" half of `/admin` (`SPEC.md`, "User-facing features", Users
// (admin): "List and delete users, toggle admin, send and revoke invites.").
//
// Every row action is its own `useMutation`, so one pending toggle disables
// one row rather than the table, and every refusal — the last-administrator
// and self-deletion 409s of `SPEC.md`, "Users (`/api/users`)" — is shown with
// the server's own text. The checkbox is controlled by the query data and
// never by local state, so a rejected toggle simply stays where it was.

import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { useAuth } from "../../hooks";
import { deleteUser, listUsers, queryKeys, updateUser } from "../../services";
import type { User } from "../../types";
import { formatRelative } from "../../utils/format";
import { Alert } from "../Alert";
import { EmptyState } from "../EmptyState";
import { LoadingState } from "../LoadingState";
import { SectionHeader } from "../SectionHeader";
import { SubmitButton } from "../SubmitButton";
import { errorMessage } from "./errorMessage";
import { CELL, HEAD, ROW, SCROLLER, TABLE, THEAD } from "./tableStyles";

const SELF_DELETE_HINT = "You cannot delete your own account";

/** A boolean column: present, or the em dash every other missing value uses. */
function Flag({ on, label }: { on: boolean; label: string }) {
  return on ? (
    <span className="text-console-text font-mono text-xs">{label}</span>
  ) : (
    <span className="text-console-muted font-mono text-xs" aria-label="no">
      —
    </span>
  );
}

export function UsersTable() {
  const queryClient = useQueryClient();
  const { user: me } = useAuth();
  const [error, setError] = useState<string | null>(null);

  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    placeholderData: keepPreviousData,
  });

  // Both outcomes refetch: when two administrators demote each other, the one
  // that got the 409 needs the truth back, not its own optimistic reading.
  function settle(caught?: unknown) {
    setError(caught === undefined ? null : errorMessage(caught));
    void queryClient.invalidateQueries({ queryKey: queryKeys.users.all });
  }

  const toggleAdmin = useMutation({
    mutationFn: (user: User) =>
      // The endpoint takes the whole representation, so the username rides
      // along unchanged.
      updateUser(user.id, { username: user.username, admin: !user.admin }),
    onSuccess: () => {
      settle();
    },
    onError: settle,
  });

  const remove = useMutation({
    mutationFn: (user: User) => deleteUser(user.id),
    onSuccess: () => {
      settle();
    },
    onError: settle,
  });

  const rows = useMemo(
    () =>
      [...(users.data ?? [])].sort((a, b) =>
        a.username.localeCompare(b.username),
      ),
    [users.data],
  );

  function onToggle(user: User) {
    if (
      user.id === me?.id &&
      user.admin &&
      !window.confirm("Remove your own administrator role?")
    ) {
      return;
    }
    toggleAdmin.mutate(user);
  }

  function onDelete(user: User) {
    if (
      !window.confirm(
        `Delete user ${user.username}? Their sessions and secrets remain attributed to a removed user.`,
      )
    ) {
      return;
    }
    remove.mutate(user);
  }

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Users"
        description="Everyone with an account. At least one administrator must remain."
      />

      {error !== null && (
        <Alert
          kind="error"
          onDismiss={() => {
            setError(null);
          }}
        >
          {error}
        </Alert>
      )}

      {users.isError && (
        <Alert kind="error">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>{`Could not load users. ${errorMessage(users.error)}`}</span>
            <SubmitButton
              type="button"
              variant="ghost"
              loading={users.isFetching}
              onClick={() => {
                void users.refetch();
              }}
            >
              Try again
            </SubmitButton>
          </div>
        </Alert>
      )}

      {users.isPending ? (
        <LoadingState />
      ) : rows.length === 0 ? (
        <EmptyState
          title="No users"
          description="Invite someone below to get started."
        />
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
              {rows.map((user) => {
                const isSelf = user.id === me?.id;
                const busy =
                  (toggleAdmin.isPending &&
                    toggleAdmin.variables?.id === user.id) ||
                  (remove.isPending && remove.variables?.id === user.id);

                return (
                  <tr key={user.id} className={ROW}>
                    <td className={`${CELL} text-console-text font-mono`}>
                      {user.username}
                      {isSelf && (
                        <span className="text-console-muted"> (you)</span>
                      )}
                    </td>
                    <td className={`${CELL} text-console-muted min-w-0`}>
                      {user.email}
                    </td>
                    <td className={CELL}>
                      <input
                        type="checkbox"
                        checked={user.admin}
                        disabled={busy}
                        aria-label={`Administrator: ${user.username}`}
                        onChange={() => {
                          onToggle(user);
                        }}
                        className="accent-console-accent size-4 align-middle disabled:opacity-50"
                      />
                    </td>
                    <td className={`${CELL} hidden sm:table-cell`}>
                      <Flag on={user.must_change_password} label="required" />
                    </td>
                    <td className={`${CELL} hidden sm:table-cell`}>
                      <Flag on={user.notify_email} label="on" />
                    </td>
                    <td
                      className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap md:table-cell`}
                    >
                      {formatRelative(user.created_at)}
                    </td>
                    <td className={`${CELL} pr-0 text-right`}>
                      <span title={isSelf ? SELF_DELETE_HINT : undefined}>
                        <SubmitButton
                          type="button"
                          variant="danger"
                          loading={
                            remove.isPending &&
                            remove.variables?.id === user.id
                          }
                          disabled={isSelf || busy}
                          onClick={() => {
                            onDelete(user);
                          }}
                        >
                          Delete
                        </SubmitButton>
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
