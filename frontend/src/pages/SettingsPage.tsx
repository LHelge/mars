// `/settings` (`SPEC.md`, "Frontend", Routes: own password and
// `notify_email`), the only page a non-administrator has for their own
// account. It calls exactly three endpoints and nothing else:
// `GET /users/me`, `PATCH /users/me` and — through `PasswordChangeForm` —
// `POST /users/{id}/password` (`SPEC.md`, "Users (`/api/users`)").
//
// The escalation opt-out is the one of `SPEC.md`, "User-facing features":
// "Every escalation into the human state emails the task's assignee, or every
// admin when there is none; each user can opt out", stored as
// `users.notify_email` (`docs/data-model.md`).
//
// There is one current user, the one in `services/auth` that `PageLayout`,
// `AdminRoute` and `ProtectedRoute` read, and this page reads it through
// `useAuth()` like everything else (`SPEC.md`, "Frontend", Rules). The query
// below is the read, not a second copy: `refreshCurrentUser` installs what
// `GET /users/me` answered, and the query itself is here for its retry and its
// error state. Nothing ever writes a cached user back into the store, which is
// what used to give a demoted administrator their Admin nav entry back from a
// five-minute-old answer.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  Alert,
  LoadingState,
  PageLayout,
  PasswordChangeForm,
  SectionHeader,
} from "../components";
import { useAuth } from "../hooks";
import { getCurrentUser, setCurrentUser } from "../services/auth";
import { refreshCurrentUser } from "../services/currentUser";
import { errorMessage } from "../services/errorMessage";
import { queryKeys } from "../services/queryKeys";
import { updateMe } from "../services/users";
import type { User } from "../types";
import { formatDateTime } from "../utils/format";

/** How long a "saved" banner stays on screen before it fades out again. */
export const CONFIRMATION_MS = 4_000;

/** The user `onMutate` captured, so `onError` can put it back. */
interface Rollback {
  previous: User | null;
}

function Field({ label, children }: { label: string; children: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-console-muted text-xs">{label}</dt>
      <dd className="text-console-text text-sm">{children}</dd>
    </div>
  );
}

export function SettingsPage() {
  const queryClient = useQueryClient();
  const key = queryKeys.users.me();
  const { user } = useAuth();

  // The signed-in user is already in memory from the authenticated bootstrap,
  // so the page paints immediately and this read is only about replacing that
  // snapshot with the authoritative one.
  const me = useQuery({
    queryKey: key,
    queryFn: refreshCurrentUser,
  });

  const [passwordChanged, setPasswordChanged] = useState(false);

  const preference = useMutation<User, Error, boolean, Rollback>({
    mutationFn: (notify_email: boolean) => updateMe({ notify_email }),
    onMutate: async (notify_email) => {
      // An in-flight `GET /users/me` would otherwise land on top of the
      // optimistic value and flip the checkbox back under the pointer.
      await queryClient.cancelQueries({ queryKey: key });
      const previous = getCurrentUser();
      if (previous !== null) {
        setCurrentUser({ ...previous, notify_email });
      }
      return { previous };
    },
    onError: (_error, _notifyEmail, context) => {
      // 400, 403, 5xx or an unreachable orchestrator alike: the checkbox goes
      // back to what the server last confirmed and the message is shown
      // verbatim.
      if (context !== undefined && context.previous !== null) {
        setCurrentUser(context.previous);
      }
    },
    onSuccess: (updated) => {
      setCurrentUser(updated);
    },
  });

  // The confirmation is the mutation's own success, so it needs no second
  // copy in state; the timer simply drops it again.
  const { isSuccess, reset } = preference;
  useEffect(() => {
    if (!isSuccess) {
      return;
    }
    const timer = setTimeout(reset, CONFIRMATION_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [isSuccess, reset]);

  return (
    <PageLayout title="Settings">
      <div className="max-w-2xl space-y-8">
        <section className="space-y-3">
          <SectionHeader
            title="Account"
            description="Your details, as the orchestrator holds them."
          />

          {user === null ? (
            me.isError ? (
              <Alert kind="error">
                {errorMessage(me.error, "Could not load your account.")}
              </Alert>
            ) : (
              <LoadingState />
            )
          ) : (
            <dl className="grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
              <div className="flex flex-col gap-0.5">
                <dt className="text-console-muted text-xs">Username</dt>
                <dd className="text-console-text font-mono text-sm">
                  {user.username}
                </dd>
              </div>
              <Field label="Email">{user.email}</Field>
              <Field label="Role">
                {user.admin ? "Administrator" : "Member"}
              </Field>
              <Field label="Member since">
                {formatDateTime(user.created_at)}
              </Field>
            </dl>
          )}
        </section>

        <section className="space-y-3">
          <SectionHeader
            title="Notifications"
            description="Escalation mail is the only mail Mars sends you."
          />

          {preference.isError && (
            <Alert kind="error">
              {errorMessage(
                preference.error,
                "Could not save your preferences.",
              )}
            </Alert>
          )}

          {preference.isSuccess && (
            <Alert kind="success">Preferences saved</Alert>
          )}

          <label className="flex max-w-prose items-start gap-2 text-sm">
            {/* Disabled while the PATCH is in flight: two overlapping writes
                would be settled by whichever answered last. */}
            <input
              type="checkbox"
              className="accent-console-accent mt-0.5 size-4 shrink-0"
              checked={user?.notify_email ?? false}
              disabled={user === null || preference.isPending}
              onChange={(event) => {
                preference.mutate(event.target.checked);
              }}
            />
            <span className="text-console-text">
              Email me when a task I am assigned to, or any task when I am an
              administrator without an assignee, needs a human
            </span>
          </label>
        </section>

        <section className="space-y-3">
          <SectionHeader
            title="Password"
            description="Changing it signs out every other session of your account."
          />

          {passwordChanged && (
            <Alert kind="success">
              Password changed. Other sessions of your account have been signed
              out.
            </Alert>
          )}

          {/* No navigation: the form installs the replacement token pair, so
              this page simply stays where it is. */}
          <PasswordChangeForm
            onSuccess={() => {
              setPasswordChanged(true);
            }}
          />
        </section>
      </div>
    </PageLayout>
  );
}
