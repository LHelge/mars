// Pushing a ref upstream (`SPEC.md`, "Git": `POST /projects/{pid}/git/push`).
//
// A push is the one git action here that leaves Mars, so the two things that
// can surprise an operator are made loud: a force push has to be asked for and
// says what it costs, and a rejected push is explained as what it is — the
// upstream moved, nothing local was lost (`README.md`, "Operating notes").

import { useState } from "react";
import { ApiError } from "../../services/apiClient";
import { push } from "../../services/git";
import type { PushResult } from "../../types";
import { shortSha } from "../../utils/format";
import { Alert } from "../Alert";
import { FormField } from "../FormField";
import { SubmitButton } from "../SubmitButton";
import {
  compareUrlFor,
  defaultRemoteBranch,
  useGitAction,
  useReportBusy,
} from "./formState";
import type { ReportBusy } from "./formState";
import { GitFormShell, GitResultNote } from "./GitFormShell";
import { ReadOnlyField } from "./ReadOnlyField";

export interface PushFormProps {
  projectId: string;
  /** A session id or an integration branch name. */
  gitRef: string;
  /** Session refs push as `session/<id>`; a head keeps its own name. */
  isSession: boolean;
  /** The ref's name, when `gitRef` is an opaque session id. */
  refLabel?: string;
  /** The project's `remote_url`, for the compare link. */
  remoteUrl: string;
  /** The branch a pushed ref is compared against; the project default. */
  compareTarget: string | null;
  formId: string;
  disabled: boolean;
  onBusy: ReportBusy;
  /** The push result and the compare link it earned, for the branch row. */
  onPushed: (result: PushResult, compareUrl: string | null) => void;
}

export function PushForm({
  projectId,
  gitRef,
  isSession,
  refLabel,
  remoteUrl,
  compareTarget,
  formId,
  disabled,
  onBusy,
  onPushed,
}: PushFormProps) {
  const [remoteBranch, setRemoteBranch] = useState(
    defaultRemoteBranch(gitRef, isSession),
  );
  const [force, setForce] = useState(false);
  // The branch a 409 refused, not a flag: the advice names what was rejected,
  // and editing the field afterwards must not rewrite it.
  const [rejected, setRejected] = useState<string | null>(null);

  const action = useGitAction<{ pushed: PushResult; compare: string | null }>(
    async () => {
      setRejected(null);
      // Trimmed once, here: what is sent, what the enable check measures and
      // what a refusal is reported against are the same string.
      const branch = remoteBranch.trim();
      try {
        const result = await push(projectId, {
          ref: gitRef,
          remote_branch: branch,
          // `force` is only sent when it was asked for: the server's default
          // is a safe push and an absent field is the same answer as `false`.
          ...(force ? { force: true } : {}),
        });
        const url = compareUrlFor(
          remoteUrl,
          compareTarget,
          result.remote_branch,
        );
        onPushed(result, url);
        return { pushed: result, compare: url };
      } catch (caught) {
        // A non-fast-forward (`SPEC.md`, "Git") is handled, not failed: the
        // banner below says what to do, so it reports no result rather than
        // raising a second alert beside it.
        if (caught instanceof ApiError && caught.status === 409) {
          setRejected(branch);
          return null;
        }
        throw caught;
      }
    },
  );

  useReportBusy(formId, action.loading, onBusy);

  return (
    <GitFormShell action={action}>
      <div className="grid gap-3 sm:grid-cols-2">
        <ReadOnlyField label="Ref" value={refLabel ?? gitRef} />

        <FormField
          label="Remote branch"
          name={`${formId}-remote-branch`}
          value={remoteBranch}
          onChange={setRemoteBranch}
          disabled={disabled}
          hint="Branch name on the remote; pushed with the project's git credential."
          help="git-credential"
        />
      </div>

      <label className="text-console-muted flex items-center gap-2 text-xs">
        <input
          type="checkbox"
          name={`${formId}-force`}
          checked={force}
          disabled={disabled}
          onChange={(event) => {
            setForce(event.target.checked);
          }}
          className="accent-state-failed"
        />
        Force push
      </label>

      {force && (
        <p className="text-state-failed text-xs">
          A force push overwrites whatever is on the remote branch. Commits only
          on the remote are lost.
        </p>
      )}

      <div className="flex flex-wrap items-center gap-3">
        <SubmitButton
          loading={action.loading}
          disabled={disabled || remoteBranch.trim() === ""}
        >
          Push
        </SubmitButton>
        {action.result !== null && (
          <GitResultNote>
            Pushed {action.result.pushed.remote_branch} at{" "}
            {shortSha(action.result.pushed.commit)}
          </GitResultNote>
        )}
        {action.result !== null && action.result.compare !== null && (
          <a
            href={action.result.compare}
            target="_blank"
            rel="noreferrer noopener"
            className="text-console-accent font-mono text-xs underline"
          >
            Open compare on GitHub
          </a>
        )}
      </div>

      {rejected !== null && (
        <Alert kind="warning">
          {`Push rejected: upstream has advanced. Fetch, merge origin/${rejected} and retry.`}
        </Alert>
      )}
    </GitFormShell>
  );
}
