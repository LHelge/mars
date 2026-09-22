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
import { QueryErrorAlert } from "../QueryErrorAlert";
import { SectionHeader } from "../SectionHeader";
import { TableHead } from "../TableHead";
import { TABLE, X_SCROLLER } from "../tableStyles";
import { SECRET_COLUMNS } from "./columns";
import { CreateSecretForm } from "./CreateSecretForm";
import { SecretRow } from "./SecretRow";
import {
  FORBIDDEN_SECRET_MESSAGE,
  isForbidden,
  PRECEDENCE_HELP,
  secretErrorMessage,
} from "./messages";

export interface SecretsManagerProps {
  scope: SecretScope;
  /** The project for `project`; omitted for `global` and for "my secrets". */
  scopeId?: string;
  title?: string;
  /**
   * Leaves the agent credentials out of the table. `/secrets` sets it because
   * it lists them above under their labels (`SPEC.md`, "Frontend", Agent
   * credentials); the project page's tab shows every row it has.
   */
  hideAgentCredentials?: boolean;
}

export function SecretsManager({
  scope,
  scopeId,
  title = "Secrets",
  hideAgentCredentials = false,
}: SecretsManagerProps) {
  const { user, isAdmin } = useAuth();

  const secrets = useQuery({
    queryKey: queryKeys.secrets.list(scope, scopeId),
    queryFn: () =>
      listSecrets({ scope, ...(scopeId === undefined ? {} : { scope_id: scopeId }) }),
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
    () =>
      (secrets.data ?? [])
        .filter(
          (secret) => !hideAgentCredentials || secret.credential_for === null,
        )
        .sort((a, b) => a.name.localeCompare(b.name)),
    [secrets.data, hideAgentCredentials],
  );

  const forbidden = secrets.isError && isForbidden(secrets.error);

  return (
    <section className="space-y-3">
      <SectionHeader
        title={title}
        description={PRECEDENCE_HELP}
        help="secrets"
      />

      {secrets.isError &&
        (forbidden ? (
          // A refusal, not a failure: there is nothing to try again.
          <Alert kind="error">{FORBIDDEN_SECRET_MESSAGE}</Alert>
        ) : (
          <QueryErrorAlert
            query={secrets}
            message={secretErrorMessage(secrets.error)}
          />
        ))}

      {!forbidden && <CreateSecretForm scope={scope} scopeId={scopeId} />}

      {/* A 403 leaves the table out entirely rather than claiming emptiness. */}
      {forbidden ? null : secrets.isPending ? (
        <LoadingState label="Loading secrets" />
      ) : rows.length === 0 ? (
        // Nor does any other failed read claim the scope is empty.
        secrets.isSuccess && (
          <EmptyState
            title="No secrets in this scope"
            description="Add one above; its value is stored encrypted and never shown again."
          />
        )
      ) : (
        <div className={X_SCROLLER}>
          <table className={TABLE}>
            <TableHead columns={SECRET_COLUMNS} sticky />
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
