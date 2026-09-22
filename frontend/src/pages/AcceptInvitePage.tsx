// `/invite/:token` (`SPEC.md`, "Frontend", Routes; "Auth (`/api/auth`)":
// `GET /auth/invite/{token}` answers `{email, admin, expires_at}` or 400, and
// `POST /auth/accept-invite` answers 201 `{user, access_token}` and sets the
// refresh cookie).
//
// The invitee never picks their own email — the invite carries it (ADR 0013,
// invite-only users) — so the form is a username and a password, shown next to
// the address the invite was sent to. The token stays in the route param: it is
// never rendered, copied into a query string or written to storage. The link
// itself may have come from the orchestrator's log in development (ADR 0026).
//
// The route's param is a `string | undefined` and everything below needs a
// token, so the guard is a component boundary: `AcceptInvitePage` decides
// whether there is an invitation to act on, and `AcceptInviteForm` is only ever
// mounted with one.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { Alert } from "../components/Alert";
import { AuthLayout } from "../components/AuthLayout";
import { FieldShell } from "../components/FieldShell";
import { FormField } from "../components/FormField";
import { LoadingState } from "../components/LoadingState";
import { NewPasswordFields } from "../components/NewPasswordFields";
import { useNewPassword } from "../components/newPassword";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { SubmitButton } from "../components/SubmitButton";
import { CONTROL } from "../components/fieldStyles";
import { useAuth, useFormSubmit } from "../hooks";
import { ApiError } from "../services/apiClient";
import { acceptInvite, lookupInvite } from "../services/auth";
import { UNREACHABLE, errorMessage } from "../services/errorMessage";
import { formatDateTime } from "../utils/format";

/** Every rejection of an invite token reads the same: unknown, used or expired. */
const INVALID_INVITE =
  "This invitation link is invalid, has expired or was already used. Ask an administrator for a new one.";

/**
 * The root of this page's own query key. The invite lookup belongs to the
 * visitor rather than to the account they are leaving, and the page still
 * renders it while the accept is in flight, so it is the one read the switch
 * of accounts keeps.
 */
const INVITE_KEY_ROOT = "invite";

const USERNAME_MIN_LENGTH = 3;
const USERNAME_MAX_LENGTH = 32;
/** The length half of the `users.username` rule in `docs/data-model.md`. */
const USERNAME_LENGTH_MESSAGE = "Username must be 3–32 characters";

/**
 * The one refusal this page words itself. A plain 400 — bad input, or an
 * invite consumed between the lookup and the submit — is already the server's
 * own sentence, and everything else is `errorMessage`'s one rule.
 */
function acceptFailure(caught: unknown): string {
  if (caught instanceof ApiError && caught.status === 409) {
    return "That username is already taken.";
  }
  return errorMessage(caught);
}

function DeadEnd() {
  return (
    <AuthLayout title="Accept your invitation">
      <div className="flex flex-col gap-4">
        <Alert kind="error">{INVALID_INVITE}</Alert>
        <Link
          to="/login"
          className="text-console-accent text-sm hover:underline"
        >
          Back to sign in
        </Link>
      </div>
    </AuthLayout>
  );
}

export function AcceptInvitePage() {
  const { token } = useParams<{ token: string }>();

  if (token === undefined || token === "") {
    return <DeadEnd />;
  }

  return <AcceptInviteForm token={token} />;
}

function AcceptInviteForm({ token }: { token: string }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { user } = useAuth();

  const lookup = useQuery({
    queryKey: [INVITE_KEY_ROOT, token],
    queryFn: () => lookupInvite(token),
  });

  const [username, setUsername] = useState("");
  const [usernameError, setUsernameError] = useState<string | null>(null);
  const chosen = useNewPassword();
  // Set when the orchestrator itself refuses the accept, which may mean the
  // invite expired while the form was open: the login link is then the exit.
  const [rejected, setRejected] = useState(false);

  const { submit, loading, error } = useFormSubmit(
    async () => {
      try {
        // `acceptInvite` installs the returned pair itself, replacing whatever
        // session the visitor arrived with.
        await acceptInvite({
          token,
          username: username.trim(),
          password: chosen.password,
        });
      } catch (caught) {
        setRejected(caught instanceof ApiError && caught.status === 400);
        throw caught;
      }
      // No return destination applies to an invite: it always lands on the
      // dashboard, and the previous account's cached reads go with it. They go
      // first: navigation is a transition, so the dashboard's own observers
      // may already have mounted by the time an `await navigate` resolves, and
      // clearing then would take their queries with it — leaving them
      // unreachable by any later invalidation until a remount. Everything but
      // this page's own invite lookup, which is still on screen.
      queryClient.removeQueries({
        predicate: (query) => query.queryKey[0] !== INVITE_KEY_ROOT,
      });
      await navigate("/", { replace: true });
    },
    { mapError: acceptFailure },
  );

  // A rejected token (400) is a dead end whenever it comes, since the invite
  // itself is gone; a network failure or a 5xx says nothing about the invite.
  if (lookup.error instanceof ApiError && lookup.error.status === 400) {
    return <DeadEnd />;
  }

  const invite = lookup.data;

  if (invite === undefined) {
    return (
      <AuthLayout title="Accept your invitation">
        {lookup.isPending ? (
          <LoadingState label="Checking your invitation" />
        ) : (
          <QueryErrorAlert query={lookup} message={UNREACHABLE} />
        )}
      </AuthLayout>
    );
  }

  function onSubmit(event: FormEvent) {
    event.preventDefault();

    const trimmed = username.trim();
    const nextUsernameError =
      trimmed.length < USERNAME_MIN_LENGTH ||
      trimmed.length > USERNAME_MAX_LENGTH
        ? USERNAME_LENGTH_MESSAGE
        : null;
    setUsernameError(nextUsernameError);
    // Both halves are checked whichever one fails, so every field that is
    // wrong says so at once.
    const chosenOk = chosen.validate();
    if (nextUsernameError !== null || !chosenOk) {
      return;
    }

    void submit();
  }

  return (
    <AuthLayout
      title="Accept your invitation"
      footer={
        rejected ? (
          <Link to="/login" className="text-console-accent hover:underline">
            Back to sign in
          </Link>
        ) : undefined
      }
    >
      <form onSubmit={onSubmit} className="flex flex-col gap-4" noValidate>
        {/* An admin may open an invite to try it out; say plainly what
            accepting it will do to the session they are already in. */}
        {user !== null && (
          <Alert kind="info">
            You are signed in as {user.username}; accepting will switch
            accounts.
          </Alert>
        )}

        {/* The invite is already in hand; a failed re-read is a banner over
            the half-filled form, not a page that replaces it. */}
        {lookup.isError && (
          <QueryErrorAlert query={lookup} message={UNREACHABLE} />
        )}

        {error !== null && <Alert kind="error">{error}</Alert>}

        <FieldShell
          label="Invited email"
          name="email"
          hint={`Expires ${formatDateTime(invite.expires_at)}`}
        >
          {(control) => (
            <input
              {...control}
              type="email"
              value={invite.email}
              readOnly
              className={`${CONTROL} text-console-muted`}
            />
          )}
        </FieldShell>

        {invite.admin && (
          <p className="text-console-accent text-xs">
            You will be an administrator
          </p>
        )}

        <FormField
          label="Username"
          name="username"
          value={username}
          onChange={(value) => {
            setUsername(value);
            setUsernameError(null);
          }}
          error={usernameError ?? undefined}
          hint="3–32 characters."
          autoComplete="username"
          autoFocus
          disabled={loading}
        />

        <NewPasswordFields
          fields={chosen}
          label="Password"
          confirmLabel="Repeat password"
          disabled={loading}
        />

        <SubmitButton loading={loading}>Create account</SubmitButton>
      </form>
    </AuthLayout>
  );
}
