// The write-only secrets manager of `SPEC.md`, "User-facing features",
// Secrets, for one scope: the metadata table, the create form and the per-row
// actions. `/secrets` mounts it under its scope selector and the project
// page's secrets tab mounts it with `scope="project"`, which is why the scope
// is a prop and not something this component decides.
//
// Everything on screen is metadata. A value exists only inside the create form
// or a replace form, only while its request is in flight (`CLAUDE.md`, rule 3).

import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { useAuth } from "../../hooks/useAuth";
import { queryKeys } from "../../services/queryKeys";
import { listSecrets } from "../../services/secrets";
import { listUsers } from "../../services/users";
import type { SecretScope } from "../../types";
import { Alert } from "../Alert";
import { EmptyState } from "../EmptyState";
import { LoadingState } from "../LoadingState";
import { SectionHeader } from "../SectionHeader";
import { SubmitButton } from "../SubmitButton";
import { CreateSecretForm } from "./CreateSecretForm";
import { SecretRow } from "./SecretRow";
import {
  FORBIDDEN_SECRET_MESSAGE,
  isForbidden,
  PRECEDENCE_HELP,
  secretErrorMessage,
} from "./messages";

const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";

export interface SecretsManagerProps {
  scope: SecretScope;
  /** The project for `project`; omitted for `global` and for "my secrets". */
  scopeId?: string;
  title?: string;
}

export function SecretsManager({
  scope,
  scopeId,
  title = "Secrets",
}: SecretsManagerProps) {
  const { user, isAdmin } = useAuth();

  const secrets = useQuery({
    queryKey: queryKeys.secrets.list(scope, scopeId),
    queryFn: () =>
      listSecrets({ scope, ...(scopeId === undefined ? {} : { scope_id: scopeId }) }),
    retry: false,
  });

  // `GET /users` is admin only; without it a `created_by` that is not the
  // viewer stays an em dash rather than a raw id.
  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    enabled: isAdmin,
  });

  const usernames = useMemo(() => {
    const map = new Map<string, string>(
      (users.data ?? []).map((u) => [u.id, u.username]),
    );
    if (user !== null && !map.has(user.id)) {
      map.set(user.id, user.username);
    }
    return map;
  }, [users.data, user]);

  const rows = useMemo(
    () => [...(secrets.data ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [secrets.data],
  );

  const forbidden = secrets.isError && isForbidden(secrets.error);

  return (
    <section className="space-y-3">
      <SectionHeader title={title} description={PRECEDENCE_HELP} />

      {secrets.isError && (
        <Alert kind="error">
          {forbidden ? (
            FORBIDDEN_SECRET_MESSAGE
          ) : (
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span>{secretErrorMessage(secrets.error)}</span>
              <SubmitButton
                type="button"
                variant="ghost"
                loading={secrets.isFetching}
                onClick={() => {
                  void secrets.refetch();
                }}
              >
                Try again
              </SubmitButton>
            </div>
          )}
        </Alert>
      )}

      {!forbidden && <CreateSecretForm scope={scope} scopeId={scopeId} />}

      {/* A 403 leaves the table out entirely rather than claiming emptiness. */}
      {forbidden ? null : secrets.isPending ? (
        <LoadingState label="Loading secrets" />
      ) : rows.length === 0 ? (
        <EmptyState
          title="No secrets in this scope"
          description="Add one above; its value is stored encrypted and never shown again."
        />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead className="bg-console-bg sticky top-0 z-10">
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  Name
                </th>
                <th scope="col" className={HEAD}>
                  Orchestrator only
                </th>
                <th scope="col" className={HEAD}>
                  Key
                </th>
                <th scope="col" className={`${HEAD} hidden lg:table-cell`}>
                  Created by
                </th>
                <th scope="col" className={`${HEAD} hidden md:table-cell`}>
                  Created
                </th>
                <th scope="col" className={`${HEAD} hidden md:table-cell`}>
                  Updated
                </th>
                <th scope="col" className={HEAD}>
                  Last used
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((secret) => (
                <SecretRow
                  key={secret.id}
                  secret={secret}
                  scope={scope}
                  scopeId={scopeId}
                  usernames={usernames}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
