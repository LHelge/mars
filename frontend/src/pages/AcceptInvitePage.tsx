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

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router";
import {
  Alert,
  AuthLayout,
  CONTROL,
  FieldShell,
  FormField,
  LoadingState,
  QueryErrorAlert,
  SubmitButton,
} from "../components";
import { useAuth, useFormSubmit } from "../hooks";
import { acceptInvite, ApiError, lookupInvite } from "../services";
import { formatDateTime, validatePassword } from "../utils";

/** Every rejection of an invite token reads the same: unknown, used or expired. */
const INVALID_INVITE =
  "This invitation link is invalid, has expired or was already used. Ask an administrator for a new one.";

const UNREACHABLE = "Orchestrator unreachable";

const USERNAME_MIN_LENGTH = 3;
const USERNAME_MAX_LENGTH = 32;
/** The length half of the `users.username` rule in `docs/data-model.md`. */
const USERNAME_LENGTH_MESSAGE = "Username must be 3–32 characters";

/**
 * Turns a failed accept into the message the user should read. Everything the
 * page words differently from the server is rewrapped as an `ApiError`, the one
 * shape `useFormSubmit` renders verbatim; a plain 400 (bad input, or an invite
 * consumed between the lookup and the submit) is already the server's own
 * sentence and passes through untouched.
 */
function acceptFailure(caught: unknown): unknown {
  if (caught instanceof ApiError) {
    if (caught.status === 409) {
      return new ApiError(caught.status, "That username is already taken.");
    }
    return caught;
  }
  if (caught instanceof TypeError) {
    // `fetch` rejects with a `TypeError` when it never reached the server.
    return new ApiError(0, UNREACHABLE);
  }
  return caught;
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
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { user } = useAuth();

  const hasToken = token !== undefined && token !== "";

  const lookup = useQuery({
    queryKey: ["invite", token],
    queryFn: () => lookupInvite(token ?? ""),
    enabled: hasToken,
  });

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [usernameError, setUsernameError] = useState<string | null>(null);
  const [passwordError, setPasswordError] = useState<string | null>(null);
  const [confirmError, setConfirmError] = useState<string | null>(null);
  // Set when the orchestrator itself refuses the accept, which may mean the
  // invite expired while the form was open: the login link is then the exit.
  const [rejected, setRejected] = useState(false);

  const { submit, loading, error } = useFormSubmit(async () => {
    try {
      // `acceptInvite` installs the returned pair itself, replacing whatever
      // session the visitor arrived with.
      await acceptInvite({
        token: token ?? "",
        username: username.trim(),
        password,
      });
    } catch (caught) {
      setRejected(caught instanceof ApiError && caught.status === 400);
      throw acceptFailure(caught);
    }
    // No return destination applies to an invite: it always lands on the
    // dashboard, and the previous account's cached reads go with it.
    await navigate("/", { replace: true });
    queryClient.clear();
  });

  if (!hasToken) {
    return <DeadEnd />;
  }

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
    const nextPasswordError = validatePassword(password);
    const nextConfirmError =
      nextPasswordError === null && confirm !== password
        ? "Passwords do not match"
        : null;
    setUsernameError(nextUsernameError);
    setPasswordError(nextPasswordError);
    setConfirmError(nextConfirmError);
    if (
      nextUsernameError !== null ||
      nextPasswordError !== null ||
      nextConfirmError !== null
    ) {
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

        {error && <Alert kind="error">{error}</Alert>}

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

        <FormField
          label="Password"
          name="password"
          type="password"
          value={password}
          onChange={(value) => {
            setPassword(value);
            setPasswordError(null);
          }}
          error={passwordError ?? undefined}
          hint="10–128 characters."
          autoComplete="new-password"
          disabled={loading}
        />

        <FormField
          label="Repeat password"
          name="confirm"
          type="password"
          value={confirm}
          onChange={(value) => {
            setConfirm(value);
            setConfirmError(null);
          }}
          error={confirmError ?? undefined}
          autoComplete="new-password"
          disabled={loading}
        />

        <SubmitButton loading={loading}>Create account</SubmitButton>
      </form>
    </AuthLayout>
  );
}
