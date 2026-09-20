// Pushing a ref upstream (`SPEC.md`, "Git": `POST /projects/{pid}/git/push`).
//
// A push is the one git action here that leaves Mars, so the two things that
// can surprise an operator are made loud: a force push has to be asked for and
// says what it costs, and a rejected push is explained as what it is — the
// upstream moved, nothing local was lost (`README.md`, "Operating notes").

import { useState } from "react";
import { useFormSubmit } from "../../hooks/useFormSubmit";
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
  useReportBusy,
} from "./formState";
import type { ReportBusy } from "./formState";

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
  const [pushed, setPushed] = useState<PushResult | null>(null);
  const [compare, setCompare] = useState<string | null>(null);
  const [rejected, setRejected] = useState(false);

  const form = useFormSubmit(async () => {
    setPushed(null);
    setCompare(null);
    setRejected(false);
    try {
      const result = await push(projectId, {
        ref: gitRef,
        remote_branch: remoteBranch,
        // `force` is only sent when it was asked for: the server's default is
        // a safe push and an absent field is the same answer as `false`.
        ...(force ? { force: true } : {}),
      });
      setPushed(result);
      const url = compareUrlFor(remoteUrl, compareTarget, result.remote_branch);
      setCompare(url);
      onPushed(result, url);
    } catch (caught) {
      if (caught instanceof ApiError && caught.status === 409) {
        setRejected(true);
      }
      throw caught;
    }
  });

  useReportBusy(formId, form.loading, onBusy);

  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        void form.submit();
      }}
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1.5">
          <span className="text-console-muted text-xs">Ref</span>
          <p className="text-console-text truncate py-1.5 font-mono text-sm">
            {refLabel ?? gitRef}
          </p>
        </div>

        <FormField
          label="Remote branch"
          name={`${formId}-remote-branch`}
          value={remoteBranch}
          onChange={setRemoteBranch}
          disabled={disabled}
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
          loading={form.loading}
          disabled={disabled || remoteBranch.trim() === ""}
        >
          Push
        </SubmitButton>
        {pushed !== null && (
          <span className="text-state-running font-mono text-xs">
            Pushed {pushed.remote_branch} at {shortSha(pushed.commit)}
          </span>
        )}
        {compare !== null && (
          <a
            href={compare}
            target="_blank"
            rel="noreferrer noopener"
            className="text-console-accent font-mono text-xs underline"
          >
            Open compare on GitHub
          </a>
        )}
      </div>

      {rejected && (
        <Alert kind="warning">
          {`Push rejected: upstream has advanced. Fetch, merge origin/${remoteBranch} and retry.`}
        </Alert>
      )}
      {form.error !== null && (
        <Alert kind="error" onDismiss={form.clearError}>
          {form.error}
        </Alert>
      )}
    </form>
  );
}
