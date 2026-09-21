// One row of the users table, with the two actions it offers: the admin
// toggle (`PUT /users/{id}`) and the deletion (`DELETE /users/{id}`).
//
// The mutations belong to the row rather than to the table, because a
// `useMutation` observer follows only its latest `mutate()`: one shared per
// action would hand its pending state and its refusal to whichever row was
// pressed last, and the row still waiting for its own request would re-enable
// itself and answer a second press with a 404. Each row is therefore its own
// observer, and the refusals of `SPEC.md`, "Users (`/api/users`)" — the
// last-administrator and self-deletion 409s — are shown under the row they
// were pressed on, in the server's own words.
//
// Both actions clear each other's answer before they start, so the one line
// this row keeps always describes the press the user is waiting on
// (`CLAUDE.md`, "Frontend conventions", Submitting a form).

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { deleteUser, queryKeys, updateUser } from "../../services";
import { errorMessage, logUnexpected } from "../../services/errorMessage";
import type { User } from "../../types";
import { formatRelative } from "../../utils/format";
import { Alert } from "../Alert";
import { SubmitButton } from "../SubmitButton";
import { CELL, ROW } from "./tableStyles";

/** The number of columns the users table has, for the answer row's span. */
const COLUMNS = 7;

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

export interface UserRowProps {
  user: User;
  /** The signed-in administrator's own row: it cannot be deleted. */
  isSelf: boolean;
}

export function UserRow({ user, isSelf }: UserRowProps) {
  const queryClient = useQueryClient();

  // Both outcomes refetch: when two administrators demote each other, the one
  // that got the 409 needs the truth back, not its own optimistic reading.
  function settle(caught?: unknown) {
    if (caught !== undefined) {
      logUnexpected(caught);
    }
    void queryClient.invalidateQueries({ queryKey: queryKeys.users.all });
  }

  const toggleAdmin = useMutation({
    mutationFn: () =>
      // The endpoint takes the whole representation, so the username rides
      // along unchanged.
      updateUser(user.id, { username: user.username, admin: !user.admin }),
    onSuccess: () => {
      settle();
    },
    onError: settle,
  });

  const remove = useMutation({
    mutationFn: () => deleteUser(user.id),
    onSuccess: () => {
      settle();
    },
    onError: settle,
  });

  const busy = toggleAdmin.isPending || remove.isPending;
  const failure = toggleAdmin.error ?? remove.error;

  function onToggle() {
    if (
      isSelf &&
      user.admin &&
      !window.confirm("Remove your own administrator role?")
    ) {
      return;
    }
    remove.reset();
    toggleAdmin.mutate();
  }

  function onDelete() {
    if (
      !window.confirm(
        `Delete user ${user.username}? Their sessions and secrets remain attributed to a removed user.`,
      )
    ) {
      return;
    }
    toggleAdmin.reset();
    remove.mutate();
  }

  return (
    <>
      <tr className={ROW}>
        <td className={`${CELL} text-console-text font-mono`}>
          {user.username}
          {isSelf && <span className="text-console-muted"> (you)</span>}
        </td>
        <td className={`${CELL} text-console-muted min-w-0`}>{user.email}</td>
        <td className={CELL}>
          <input
            type="checkbox"
            checked={user.admin}
            disabled={busy}
            aria-label={`Administrator: ${user.username}`}
            onChange={onToggle}
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
          <SubmitButton
            type="button"
            variant="danger"
            loading={remove.isPending}
            disabled={isSelf || busy}
            title={isSelf ? SELF_DELETE_HINT : undefined}
            onClick={onDelete}
          >
            Delete
          </SubmitButton>
        </td>
      </tr>

      {failure !== null && (
        <tr className={ROW}>
          <td colSpan={COLUMNS} className="px-0 pb-2">
            <Alert kind="error">{errorMessage(failure)}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}
