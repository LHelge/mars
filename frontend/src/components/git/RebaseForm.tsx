// Rebasing a branch onto another ref (`SPEC.md`, "Git": `POST
// /projects/{pid}/git/rebase`).
//
// Rebasing the branch of a session that is still working is allowed, and the
// work clone only follows when its tree is clean (`ARCHITECTURE.md`, "Git
// model"), so the form says so before the button rather than after the fact.

import { useState } from "react";
import { rebase } from "../../services/git";
import type { Branch } from "../../types";
import { shortSha } from "../../utils/format";
import { Alert } from "../Alert";
import { FieldShell } from "../FieldShell";
import { FIELD } from "../fieldStyles";
import { HelpLink } from "../HelpLink";
import { SubmitButton } from "../SubmitButton";
import { chosenOr, refsOfKind, useGitAction, useReportBusy } from "./formState";
import type { ReportBusy } from "./formState";
import { GitFormShell, GitResultNote } from "./GitFormShell";
import { ReadOnlyField } from "./ReadOnlyField";
import { RefOptions } from "./RefOptions";

/** `ARCHITECTURE.md`, "Git model": a dirty work tree is left for the agent. */
const WORK_TREE_HINT =
  "A running session's checkout follows the rebase only if it has no uncommitted changes; otherwise the session's git event reports that it needs reconciling.";

export interface RebaseFormProps {
  projectId: string;
  branches: Branch[];
  /** A session id or an integration branch name. */
  branch: string;
  /** The branch's name, when `branch` is an opaque session id. */
  branchLabel?: string;
  /** Preselected `onto`; the project's default branch. */
  defaultOnto?: string;
  formId: string;
  disabled: boolean;
  onBusy: ReportBusy;
  onRebased: () => void;
  /**
   * The outcome the session's own `git` event reported, when the session view
   * has one: a rebase that could not touch a dirty checkout says so there.
   */
  workTreeNote?: string | null;
}

export function RebaseForm({
  projectId,
  branches,
  branch,
  branchLabel,
  defaultOnto,
  formId,
  disabled,
  onBusy,
  onRebased,
  workTreeNote,
}: RebaseFormProps) {
  const heads = refsOfKind(branches, "head");
  const upstream = refsOfKind(branches, "upstream");
  const [chosenOnto, setChosenOnto] = useState(defaultOnto ?? "");
  const onto = chosenOr(chosenOnto, [...heads, ...upstream]);

  const action = useGitAction(async () => {
    const result = await rebase(projectId, { branch, onto });
    onRebased();
    return result.commit;
  });

  useReportBusy(formId, action.loading, onBusy);

  return (
    <GitFormShell action={action}>
      <div className="grid gap-3 sm:grid-cols-2">
        <ReadOnlyField label="Branch" value={branchLabel ?? branch} />

        <FieldShell label="Onto" name={`${formId}-onto`}>
          {(control) => (
            <select
              {...control}
              value={onto}
              disabled={disabled}
              onChange={(event) => {
                setChosenOnto(event.target.value);
              }}
              className={FIELD}
            >
              {heads.length === 0 && upstream.length === 0 && (
                <option value="">No ref to rebase onto</option>
              )}
              <RefOptions
                branches={branches}
                kind="head"
                label="Integration heads"
              />
              <RefOptions
                branches={branches}
                kind="upstream"
                label="Upstream"
              />
            </select>
          )}
        </FieldShell>
      </div>

      <p className="text-console-muted text-xs">
        {WORK_TREE_HINT} <HelpLink topic="branches" />
      </p>

      <div className="flex items-center gap-3">
        <SubmitButton
          loading={action.loading}
          disabled={disabled || onto === ""}
        >
          Rebase
        </SubmitButton>
        {action.result !== null && (
          <GitResultNote>Rebased to {shortSha(action.result)}</GitResultNote>
        )}
      </div>

      {workTreeNote !== null && workTreeNote !== undefined && (
        <Alert kind="warning">{workTreeNote}</Alert>
      )}
    </GitFormShell>
  );
}
