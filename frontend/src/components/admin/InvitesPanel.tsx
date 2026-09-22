// The "Invitations" half of `/admin` (`SPEC.md`, "User-facing features",
// Login and invites: invites expire after 7 days and can be revoked, and the
// link is delivered by Resend or written to the orchestrator log).
//
// The invitation token is never part of any response (`SPEC.md`, "Users
// (`/api/users`)"), so it appears nowhere on this page. The description and
// the success message say where the link goes in either case — the page
// cannot tell whether email is configured — which is the whole of ADR 0026
// from an operator's side.
//
// This panel owns the invitation form and nothing else: resending and revoking
// are `InviteRow`'s, one observer and one answer line per row, so the panel's
// banners always describe the form the user just submitted.

import {
  keepPreviousData,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { useFormSubmit } from "../../hooks";
import { ApiError } from "../../services/apiClient";
import { queryKeys } from "../../services/queryKeys";
import { createInvite, listInvites, listUsers } from "../../services/users";
import type { Invite } from "../../types";
import { Alert } from "../Alert";
import { EmptyState } from "../EmptyState";
import { FormField } from "../FormField";
import { LoadingState } from "../LoadingState";
import { QueryErrorAlert } from "../QueryErrorAlert";
import { SectionHeader } from "../SectionHeader";
import { SubmitButton } from "../SubmitButton";
import { errorMessage } from "../../services/errorMessage";
import { TableHead } from "../TableHead";
import { SCROLLER, TABLE } from "../tableStyles";
import { INVITE_COLUMNS } from "./columns";
import { INVITE_DELIVERY } from "./inviteDelivery";
import { InviteRow } from "./InviteRow";

const DUPLICATE = "That email already has an account or an open invitation.";

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
  // Which address the last successful submission invited; the banner is shown
  // by `succeeded`, this only says who it went to.
  const [sentTo, setSentTo] = useState<string | null>(null);

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

  const { submit, loading, error, succeeded, reset } = useFormSubmit(
    async () => {
      // The server lower-cases and trims the address; matching it here keeps
      // the duplicate check predictable.
      const normalised = email.trim().toLowerCase();

      const invite = await createInvite({ email: normalised, admin });

      queryClient.setQueryData<Invite[]>(queryKeys.invites.list(), (rows) => [
        invite,
        ...(rows ?? []),
      ]);
      await queryClient.invalidateQueries({ queryKey: queryKeys.invites.all });
      setEmail("");
      setAdmin(false);
      setSentTo(invite.email);
    },
    { mapError: inviteFailure },
  );

  function onSubmit(event: FormEvent) {
    event.preventDefault();
    void submit();
  }

  const rows = invites.data ?? [];

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Invitations"
        description={`An invitation link is good for 7 days. ${INVITE_DELIVERY} Revoking one takes it out of use immediately.`}
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
              // The last answer described the address that has just been
              // edited away.
              reset();
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
      {succeeded && sentTo !== null && (
        <Alert kind="success">
          {`Invitation created for ${sentTo}. ${INVITE_DELIVERY}`}
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
            <TableHead columns={INVITE_COLUMNS} sticky />
            <tbody>
              {rows.map((invite) => (
                <InviteRow
                  key={invite.id}
                  invite={invite}
                  invitedBy={
                    invite.invited_by === null
                      ? null
                      : (usernamesById.get(invite.invited_by) ?? null)
                  }
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
