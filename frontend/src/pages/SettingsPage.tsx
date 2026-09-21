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
// Two copies of the current user exist — `services/auth`'s, which
// `PageLayout`, `AdminRoute` and `ProtectedRoute` read, and the query cache's
// — so whichever `User` arrives last is written to both: the fetch, the
// preference mutation and (inside the form) the password change all install
// the same object. A user demoted elsewhere therefore loses the Admin nav
// entry as soon as this page reads `GET /users/me`.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  Alert,
  LoadingState,
  PageLayout,
  PasswordChangeForm,
  SectionHeader,
} from "../components";
import { getCurrentUser, setCurrentUser } from "../services/auth";
import { errorMessage } from "../services/errorMessage";
import { queryKeys } from "../services/queryKeys";
import { getMe, updateMe } from "../services/users";
import type { User } from "../types";
import { formatDateTime } from "../utils/format";

/** How long a "saved" banner stays on screen before it fades out again. */
export const CONFIRMATION_MS = 4_000;

/** The cache value `onMutate` captured, so `onError` can put it back. */
interface Rollback {
  previous: User | undefined;
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

  // The signed-in user is already in memory from the authenticated bootstrap,
  // so the page paints immediately; `initialDataUpdatedAt: 0` marks that copy
  // as stale, which refetches `GET /users/me` at once for the authoritative
  // one.
  const me = useQuery({
    queryKey: key,
    queryFn: getMe,
    initialData: () => getCurrentUser() ?? undefined,
    initialDataUpdatedAt: 0,
  });

  const user = me.data;

  // Whatever the fetch answered wins over the bootstrap snapshot everywhere.
  useEffect(() => {
    if (user !== undefined) {
      setCurrentUser(user);
    }
  }, [user]);

  const [savedAt, setSavedAt] = useState<number | null>(null);
  const [passwordChanged, setPasswordChanged] = useState(false);

  useEffect(() => {
    if (savedAt === null) {
      return;
    }
    const timer = setTimeout(() => {
      setSavedAt(null);
    }, CONFIRMATION_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [savedAt]);

  const preference = useMutation<User, Error, boolean, Rollback>({
    mutationFn: (notify_email: boolean) => updateMe({ notify_email }),
    onMutate: async (notify_email) => {
      // An in-flight `GET /users/me` would otherwise land on top of the
      // optimistic value and flip the checkbox back under the pointer.
      await queryClient.cancelQueries({ queryKey: key });
      const previous = queryClient.getQueryData<User>(key);
      if (previous !== undefined) {
        queryClient.setQueryData<User>(key, { ...previous, notify_email });
      }
      setSavedAt(null);
      return { previous };
    },
    onError: (_error, _notifyEmail, context) => {
      // 400, 403, 5xx or an unreachable orchestrator alike: the checkbox goes
      // back to what the server last confirmed and the message is shown
      // verbatim.
      if (context?.previous !== undefined) {
        queryClient.setQueryData<User>(key, context.previous);
      }
    },
    onSuccess: (updated) => {
      queryClient.setQueryData<User>(key, updated);
      setCurrentUser(updated);
      setSavedAt(Date.now());
    },
  });

  return (
    <PageLayout title="Settings">
      <div className="max-w-2xl space-y-8">
        <section className="space-y-3">
          <SectionHeader
            title="Account"
            description="Your details, as the orchestrator holds them."
          />

          {user === undefined ? (
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
              {errorMessage(preference.error, "Could not save your preferences.")}
            </Alert>
          )}

          {savedAt !== null && <Alert kind="success">Preferences saved</Alert>}

          <label className="flex max-w-prose items-start gap-2 text-sm">
            <input
              type="checkbox"
              className="accent-console-accent mt-0.5 size-4 shrink-0"
              checked={user?.notify_email ?? false}
              disabled={user === undefined}
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
