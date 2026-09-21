// The "Invitations" half of `/admin` (`SPEC.md`, "User-facing features",
// Login and invites: invites expire after 7 days and can be revoked, and the
// link is delivered by Resend or written to the orchestrator log).
//
// The invitation token is never part of any response (`SPEC.md`, "Users
// (`/api/users`)"), so it appears nowhere on this page. The success message
// says where the link is when no email is configured, which is the whole of
// ADR 0026 from an operator's side.

import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { useFormSubmit } from "../../hooks";
import {
  ApiError,
  createInvite,
  listInvites,
  listUsers,
  queryKeys,
  resendInvite,
  revokeInvite,
} from "../../services";
import type { Invite } from "../../types";
import { formatDateTime, formatRelative, PLACEHOLDER } from "../../utils/format";
import { Alert } from "../Alert";
import { EmptyState } from "../EmptyState";
import { FormField } from "../FormField";
import { LoadingState } from "../LoadingState";
import { QueryErrorAlert } from "../QueryErrorAlert";
import { SectionHeader } from "../SectionHeader";
import { SubmitButton } from "../SubmitButton";
import { errorMessage, logUnexpected } from "../../services/errorMessage";
import { CELL, HEAD, ROW, SCROLLER, TABLE, THEAD } from "./tableStyles";

const DUPLICATE =
  "That email already has an account or an open invitation.";

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * How long is left, as `in 6d` / `in 3h` / `in 12m`. `formatRelative` measures
 * age and clamps the future away, so a deadline needs its own reading; a
 * deadline already past is handed back to `formatRelative` and shown in the
 * failed-state colour.
 */
function untilLabel(iso: string, now: Date): string | null {
  const at = new Date(iso).getTime();
  if (Number.isNaN(at)) {
    return null;
  }
  const seconds = Math.round((at - now.getTime()) / 1000);
  if (seconds <= 0) {
    return null;
  }
  if (seconds < MINUTE) {
    return "in under a minute";
  }
  if (seconds < HOUR) {
    return `in ${Math.floor(seconds / MINUTE)}m`;
  }
  if (seconds < DAY) {
    return `in ${Math.floor(seconds / HOUR)}h`;
  }
  return `in ${Math.floor(seconds / DAY)}d`;
}

/**
 * An invite the reaper has not collected yet is still listed; it reads as
 * expired and keeps its Resend action, because a resend issues a new expiry.
 */
export function Expiry({ iso, now = new Date() }: { iso: string; now?: Date }) {
  if (Number.isNaN(new Date(iso).getTime())) {
    return <span className="text-console-muted">{PLACEHOLDER}</span>;
  }
  const remaining = untilLabel(iso, now);
  return (
    <span
      title={formatDateTime(iso)}
      className={
        remaining === null
          ? "text-state-failed font-mono text-xs"
          : "text-console-muted font-mono text-xs"
      }
    >
      {remaining ?? `expired ${formatRelative(iso, now)}`}
    </span>
  );
}

/**
 * The one thing this panel says differently from the server: 400 keeps the
 * validation text and a network failure is `errorMessage`'s to name, but
 * "email already invited" does not say what to do about it.
 */
function inviteFailure(caught: unknown): string {
  if (caught instanceof ApiError && caught.status === 409) {
    return DUPLICATE;
  }
  return errorMessage(caught);
}

export function InvitesPanel() {
  const queryClient = useQueryClient();

  const [email, setEmail] = useState("");
  const [admin, setAdmin] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [rowError, setRowError] = useState<string | null>(null);

  const invites = useQuery({
    queryKey: queryKeys.invites.list(),
    queryFn: listInvites,
    placeholderData: keepPreviousData,
  });

  // Shared with the users table through the query cache, so the page reads
  // `GET /users` once.
  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    placeholderData: keepPreviousData,
  });

  const usernamesById = useMemo(
    () =>
      new Map<string, string>(
        (users.data ?? []).map((user) => [user.id, user.username]),
      ),
    [users.data],
  );

  function writeRows(next: (rows: Invite[]) => Invite[]) {
    queryClient.setQueryData<Invite[]>(queryKeys.invites.list(), (rows) =>
      next(rows ?? []),
    );
    void queryClient.invalidateQueries({ queryKey: queryKeys.invites.all });
  }

  const { submit, loading, error } = useFormSubmit(async () => {
    setNotice(null);
    setRowError(null);
    // The server lower-cases and trims the address; matching it here keeps the
    // duplicate check predictable.
    const normalised = email.trim().toLowerCase();

    const invite = await createInvite({ email: normalised, admin });

    writeRows((rows) => [invite, ...rows]);
    setEmail("");
    setAdmin(false);
    setNotice(
      `Invitation sent to ${invite.email}. Without email configured, the link is in the orchestrator log.`,
    );
  }, {
    mapError: inviteFailure,
  });

  const resend = useMutation({
    mutationFn: (invite: Invite) => resendInvite(invite.id),
    onSuccess: (updated) => {
      setRowError(null);
      writeRows((rows) =>
        rows.map((row) => (row.id === updated.id ? updated : row)),
      );
      setNotice("Invitation re-sent");
    },
    onError: (caught) => {
      logUnexpected(caught);
      setRowError(errorMessage(caught));
      void queryClient.invalidateQueries({ queryKey: queryKeys.invites.all });
    },
  });

  const revoke = useMutation({
    mutationFn: (invite: Invite) => revokeInvite(invite.id),
    onSuccess: (_void, invite) => {
      setRowError(null);
      setNotice(null);
      writeRows((rows) => rows.filter((row) => row.id !== invite.id));
    },
    onError: (caught) => {
      logUnexpected(caught);
      setRowError(errorMessage(caught));
      void queryClient.invalidateQueries({ queryKey: queryKeys.invites.all });
    },
  });

  function onSubmit(event: FormEvent) {
    event.preventDefault();
    void submit();
  }

  function onRevoke(invite: Invite) {
    if (!window.confirm(`Revoke the invitation for ${invite.email}?`)) {
      return;
    }
    revoke.mutate(invite);
  }

  const rows = invites.data ?? [];

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Invitations"
        description="An invitation link is good for 7 days. Revoking one takes it out of use immediately."
      />

      <form
        onSubmit={onSubmit}
        className="border-console-border bg-console-surface flex flex-wrap items-end gap-3 rounded border p-3"
      >
        <div className="min-w-56 flex-1">
          <FormField
            label="Email"
            name="invite-email"
            type="email"
            value={email}
            onChange={(value) => {
              setEmail(value);
            }}
            autoComplete="email"
            required
            disabled={loading}
          />
        </div>

        <label className="text-console-muted flex items-center gap-2 py-2 text-xs">
          <input
            type="checkbox"
            checked={admin}
            disabled={loading}
            onChange={(event) => {
              setAdmin(event.target.checked);
            }}
            className="accent-console-accent size-4"
          />
          Administrator
        </label>

        <SubmitButton loading={loading}>Send invitation</SubmitButton>
      </form>

      {error !== null && <Alert kind="error">{error}</Alert>}
      {rowError !== null && (
        <Alert
          kind="error"
          onDismiss={() => {
            setRowError(null);
          }}
        >
          {rowError}
        </Alert>
      )}
      {notice !== null && (
        <Alert
          kind="success"
          onDismiss={() => {
            setNotice(null);
          }}
        >
          {notice}
        </Alert>
      )}

      {invites.isError && (
        <QueryErrorAlert
          query={invites}
          message={`Could not load invitations. ${errorMessage(invites.error)}`}
        />
      )}

      {/* A failed read is not an empty list (`SPEC.md`, "Frontend", Read
          failures). */}
      {invites.isPending ? (
        <LoadingState />
      ) : rows.length === 0 ? (
        invites.isSuccess && (
          <EmptyState
            title="No open invitations"
            description="Invite someone with the form above."
          />
        )
      ) : (
        <div className={SCROLLER}>
          <table className={TABLE}>
            <thead className={THEAD}>
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  Email
                </th>
                <th scope="col" className={HEAD}>
                  Admin
                </th>
                <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
                  Invited by
                </th>
                <th scope="col" className={HEAD}>
                  Expires
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
              {rows.map((invite) => {
                const busy =
                  (resend.isPending && resend.variables?.id === invite.id) ||
                  (revoke.isPending && revoke.variables?.id === invite.id);

                return (
                  <tr key={invite.id} className={ROW}>
                    <td className={`${CELL} text-console-text min-w-0 font-mono`}>
                      {invite.email}
                    </td>
                    <td className={`${CELL} text-console-muted font-mono text-xs`}>
                      {invite.admin ? "yes" : PLACEHOLDER}
                    </td>
                    <td
                      className={`${CELL} text-console-muted hidden sm:table-cell`}
                    >
                      {(invite.invited_by === null
                        ? null
                        : usernamesById.get(invite.invited_by)) ?? PLACEHOLDER}
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
                          loading={
                            resend.isPending &&
                            resend.variables?.id === invite.id
                          }
                          disabled={busy}
                          onClick={() => {
                            resend.mutate(invite);
                          }}
                        >
                          Resend
                        </SubmitButton>
                        <SubmitButton
                          type="button"
                          variant="danger"
                          loading={
                            revoke.isPending &&
                            revoke.variables?.id === invite.id
                          }
                          disabled={busy}
                          onClick={() => {
                            onRevoke(invite);
                          }}
                        >
                          Revoke
                        </SubmitButton>
                      </div>
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
