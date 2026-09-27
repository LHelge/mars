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
//
// Both also ask first, in the row's own spanning cell rather than in a
// `window.confirm` over the whole tab (`components/ConfirmPanel.tsx`), and the
// confirming button names the user it is about: a table of them offers the
// same verb on every line.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { queryKeys } from "../../services/queryKeys";
import { deleteUser, updateUser } from "../../services/users";
import { errorMessage, logUnexpected } from "../../services/errorMessage";
import type { User } from "../../types";
import { formatRelative } from "../../utils/format";
import { Alert } from "../Alert";
import { ConfirmPanel } from "../ConfirmPanel";
import { ControlNote } from "../ControlNote";
import { SubmitButton } from "../SubmitButton";
import { CELL, ROW, SPAN_CELL_BARE } from "../tableStyles";
import { USER_COLUMNS } from "./columns";
import { CHECK_TOUCH } from "../fieldStyles";

const SELF_DELETE_HINT = "You cannot delete your own account";

/** What losing the role costs, said before it is lost. */
const ADMIN_EXIT_HINT =
  "The administration page closes at its next request and you cannot open it again yourself.";

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
  /** Which of the two actions has been asked about and not yet answered. */
  const [confirming, setConfirming] = useState<"admin" | "delete" | null>(null);

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
  const selfNoteId = `user-${user.id}-self-delete`;
  const failure = toggleAdmin.error ?? remove.error;

  /** Stepping down is the only toggle worth asking about. */
  function onToggle() {
    if (isSelf && user.admin) {
      setConfirming("admin");
      return;
    }
    remove.reset();
    toggleAdmin.mutate();
  }

  function confirmToggle() {
    setConfirming(null);
    remove.reset();
    toggleAdmin.mutate();
  }

  function confirmDelete() {
    setConfirming(null);
    toggleAdmin.reset();
    remove.mutate();
  }

  return (
    <>
      <tr className={ROW}>
        <td className={`${CELL} text-console-text font-mono`}>
          <span>{user.username}</span>
          {isSelf && <span className="text-console-muted"> (you)</span>}
          {/* Below `sm` the address rides under the name, so the row keeps
              its one action on screen (`components/tableStyles.ts`). */}
          <span className="text-console-muted block font-sans text-xs break-all sm:hidden">
            {user.email}
          </span>
        </td>
        <td
          className={`${CELL} text-console-muted hidden min-w-0 sm:table-cell`}
        >
          {user.email}
        </td>
        <td className={CELL}>
          <input
            type="checkbox"
            checked={user.admin}
            disabled={busy}
            aria-label={`Administrator: ${user.username}`}
            onChange={onToggle}
            className={`accent-console-accent size-4 align-middle disabled:opacity-50 ${CHECK_TOUCH}`}
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
            aria-describedby={isSelf ? selfNoteId : undefined}
            onClick={() => {
              setConfirming("delete");
            }}
          >
            Delete
          </SubmitButton>
          {/* Why the button is shut, as text: a phone has no tooltip. */}
          {isSelf && (
            <ControlNote id={selfNoteId} className="pt-1">
              {SELF_DELETE_HINT}
            </ControlNote>
          )}
        </td>
      </tr>

      {confirming !== null && (
        <tr className={ROW}>
          <td colSpan={USER_COLUMNS.length} className={SPAN_CELL_BARE}>
            {confirming === "admin" ? (
              <ConfirmPanel
                tone="caution"
                message={`Remove your own administrator role? ${ADMIN_EXIT_HINT}`}
                confirmLabel="Step down as administrator"
                pending={toggleAdmin.isPending}
                onConfirm={confirmToggle}
                onCancel={() => {
                  setConfirming(null);
                }}
              />
            ) : (
              <ConfirmPanel
                message={`Delete user ${user.username}? Their sessions and secrets remain attributed to a removed user.`}
                confirmLabel={`Delete user ${user.username}`}
                pending={remove.isPending}
                onConfirm={confirmDelete}
                onCancel={() => {
                  setConfirming(null);
                }}
              />
            )}
          </td>
        </tr>
      )}

      {failure !== null && (
        <tr className={ROW}>
          <td colSpan={USER_COLUMNS.length} className={SPAN_CELL_BARE}>
            <Alert kind="error">{errorMessage(failure)}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}
