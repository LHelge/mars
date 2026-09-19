// `GET /secrets/{id}/uses` (`SPEC.md`, "Secrets"): when and why a secret was
// read. A `launch` row names the session it was injected into; a `git` row
// names the user whose request needed the credential, or nothing at all when
// the orchestrator fetched a mirror on its own schedule.
//
// There is no pagination endpoint, so "Show more" doubles the limit and asks
// again, up to the 500 the server would cap at anyway.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router";
import { queryKeys } from "../../services/queryKeys";
import { listSecretUses } from "../../services/secrets";
import type { SecretUse } from "../../types";
import { formatDateTime, formatRelative } from "../../utils/format";
import { Alert } from "../Alert";
import { LoadingState } from "../LoadingState";
import { SubmitButton } from "../SubmitButton";
import { secretErrorMessage } from "./messages";

/** The first page, and the step the button doubles from. */
export const USES_INITIAL_LIMIT = 20;
/** As far as this view will go; the server's own ceiling is 500. */
export const USES_MAX_LIMIT = 200;

export interface SecretUsesListProps {
  secretId: string;
  /** Admin-resolved usernames; a `git` use falls back to the raw id. */
  usernames: Map<string, string>;
}

function actor(use: SecretUse, usernames: Map<string, string>) {
  if (use.session_id !== null) {
    return (
      <Link
        to={`/sessions/${use.session_id}`}
        className="text-console-accent font-mono text-xs hover:underline"
      >
        {use.session_id.slice(0, 8)}
      </Link>
    );
  }
  if (use.user_id !== null) {
    return (
      <span className="font-mono text-xs">
        {usernames.get(use.user_id) ?? use.user_id}
      </span>
    );
  }
  // Neither a session nor a user: the mirror refresh job.
  return <span className="text-console-muted text-xs">mirror fetch</span>;
}

export function SecretUsesList({ secretId, usernames }: SecretUsesListProps) {
  const [limit, setLimit] = useState(USES_INITIAL_LIMIT);

  const uses = useQuery({
    queryKey: queryKeys.secrets.uses(secretId, limit),
    queryFn: () => listSecretUses(secretId, limit),
  });

  if (uses.isPending) {
    return <LoadingState label="Loading uses" />;
  }

  if (uses.isError) {
    return <Alert kind="error">{secretErrorMessage(uses.error)}</Alert>;
  }

  const rows = uses.data;

  if (rows.length === 0) {
    return (
      <p className="text-console-muted text-xs">
        This secret has not been used yet.
      </p>
    );
  }

  return (
    <div className="flex flex-col items-start gap-2">
      <ul className="flex w-full flex-col gap-1">
        {rows.map((use, index) => (
          <li
            key={`${use.at}-${index}`}
            className="text-console-muted flex flex-wrap items-baseline gap-x-3 text-xs"
          >
            <span className="font-mono" title={formatDateTime(use.at)}>
              {formatRelative(use.at)}
            </span>
            <span>{use.purpose}</span>
            {actor(use, usernames)}
          </li>
        ))}
      </ul>

      {rows.length >= limit && limit < USES_MAX_LIMIT && (
        <SubmitButton
          type="button"
          variant="ghost"
          loading={uses.isFetching}
          onClick={() => {
            setLimit((current) => Math.min(current * 2, USES_MAX_LIMIT));
          }}
        >
          Show more
        </SubmitButton>
      )}
    </div>
  );
}
