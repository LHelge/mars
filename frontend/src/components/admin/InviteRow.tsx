// One row of the invitations table: resend (`POST /invites/{id}/resend`) and
// revoke (`DELETE /invites/{id}`).
//
// Each row owns its two mutations for the reason `UserRow` does: one shared
// observer per action follows only the latest press, so a second revoke would
// take the first row's spinner away while its request is still in flight. The
// row's answer — a refusal, or the confirmation that a new link went out —
// therefore sits on the row it belongs to, and each action clears the other's
// before it starts (`CLAUDE.md`, "Frontend conventions", Submitting a form).

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { queryKeys } from "../../services/queryKeys";
import { resendInvite, revokeInvite } from "../../services/users";
import { errorMessage, logUnexpected } from "../../services/errorMessage";
import type { Invite } from "../../types";
import { formatRelative, PLACEHOLDER } from "../../utils/format";
import { Alert } from "../Alert";
import { ConfirmPanel } from "../ConfirmPanel";
import { SubmitButton } from "../SubmitButton";
import { CELL, ROW, SPAN_CELL_BARE } from "../tableStyles";
import { INVITE_COLUMNS } from "./columns";
import { Expiry } from "./Expiry";

export interface InviteRowProps {
  invite: Invite;
  /** The username of `invited_by`, when the users list resolves it. */
  invitedBy: string | null;
}

export function InviteRow({ invite, invitedBy }: InviteRowProps) {
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);

  /** The row the server answered with, before the refetch arrives. */
  function writeRows(next: (rows: Invite[]) => Invite[]) {
    queryClient.setQueryData<Invite[]>(queryKeys.invites.list(), (rows) =>
      next(rows ?? []),
    );
    void queryClient.invalidateQueries({ queryKey: queryKeys.invites.all });
  }

  function onFailure(caught: unknown) {
    logUnexpected(caught);
    // The invitation may have been accepted or reaped since the list was read;
    // find out.
    void queryClient.invalidateQueries({ queryKey: queryKeys.invites.all });
  }

  const resend = useMutation({
    mutationFn: () => resendInvite(invite.id),
    onSuccess: (updated) => {
      writeRows((rows) =>
        rows.map((row) => (row.id === updated.id ? updated : row)),
      );
    },
    onError: onFailure,
  });

  const revoke = useMutation({
    mutationFn: () => revokeInvite(invite.id),
    onSuccess: () => {
      writeRows((rows) => rows.filter((row) => row.id !== invite.id));
    },
    onError: onFailure,
  });

  const busy = resend.isPending || revoke.isPending;
  const failure = resend.error ?? revoke.error;

  function onResend() {
    revoke.reset();
    resend.mutate();
  }

  function onRevoke() {
    setConfirming(false);
    resend.reset();
    revoke.mutate();
  }

  return (
    <>
      <tr className={ROW}>
        <td className={`${CELL} text-console-text min-w-0 font-mono`}>
          {invite.email}
        </td>
        <td className={`${CELL} text-console-muted font-mono text-xs`}>
          {invite.admin ? "yes" : PLACEHOLDER}
        </td>
        <td className={`${CELL} text-console-muted hidden sm:table-cell`}>
          {invitedBy ?? PLACEHOLDER}
        </td>
        <td className={`${CELL} whitespace-nowrap`}>
          <Expiry iso={invite.expires_at} />
        </td>
        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap md:table-cell`}
        >
          {formatRelative(invite.created_at)}
        </td>
        <td className={`${CELL} pr-0 text-right`}>
          <div className="flex justify-end gap-2">
            <SubmitButton
              type="button"
              variant="ghost"
              loading={resend.isPending}
              disabled={busy}
              onClick={onResend}
            >
              Resend
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="danger"
              loading={revoke.isPending}
              disabled={busy || confirming}
              onClick={() => {
                setConfirming(true);
              }}
            >
              Revoke
            </SubmitButton>
          </div>
        </td>
      </tr>

      {confirming && (
        <tr className={ROW}>
          <td colSpan={INVITE_COLUMNS.length} className={SPAN_CELL_BARE}>
            <ConfirmPanel
              message={`Revoke the invitation for ${invite.email}? The link already sent stops working.`}
              confirmLabel={`Revoke the invitation for ${invite.email}`}
              pending={revoke.isPending}
              onConfirm={onRevoke}
              onCancel={() => {
                setConfirming(false);
              }}
            />
          </td>
        </tr>
      )}

      {(failure !== null || resend.isSuccess) && (
        <tr className={ROW}>
          <td colSpan={INVITE_COLUMNS.length} className={SPAN_CELL_BARE}>
            {failure !== null ? (
              <Alert kind="error">{errorMessage(failure)}</Alert>
            ) : (
              <Alert kind="success">Invitation re-sent</Alert>
            )}
          </td>
        </tr>
      )}
    </>
  );
}
