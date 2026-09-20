// Rebasing a branch onto another ref (`SPEC.md`, "Git": `POST
// /projects/{pid}/git/rebase`).
//
// Rebasing the branch of a session that is still working is allowed, and the
// work clone only follows when its tree is clean (`ARCHITECTURE.md`, "Git
// model"), so the form says so before the button rather than after the fact.

import { useState } from "react";
import { useFormSubmit } from "../../hooks/useFormSubmit";
import { isGitConflict, rebase } from "../../services/git";
import type { Branch } from "../../types";
import { shortSha } from "../../utils/format";
import { Alert } from "../Alert";
import { FormField } from "../FormField";
import { SubmitButton } from "../SubmitButton";
import { ConflictList } from "./ConflictList";
import { chosenOr, CONTROL, refsOfKind, useReportBusy } from "./formState";
import type { ReportBusy } from "./formState";

/** `ARCHITECTURE.md`, "Git model": a dirty work tree is left for the agent. */
const WORK_TREE_HINT =
  "The session's checkout is reset onto the new commits only while it is clean; otherwise the session's git event asks for reconciliation.";

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
  const [rebased, setRebased] = useState<string | null>(null);
  const [conflicts, setConflicts] = useState<string[] | null>(null);
  const [conflictMessage, setConflictMessage] = useState("");

  const form = useFormSubmit(async () => {
    setRebased(null);
    setConflicts(null);
    try {
      const result = await rebase(projectId, { branch, onto });
      setRebased(result.commit);
      onRebased();
    } catch (caught) {
      if (isGitConflict(caught)) {
        setConflicts(caught.conflicts);
        setConflictMessage(caught.error);
        return;
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
          <span className="text-console-muted text-xs">Branch</span>
          <p className="text-console-text truncate py-1.5 font-mono text-sm">
            {branchLabel ?? branch}
          </p>
        </div>

        <FormField
          label="Onto"
          name={`${formId}-onto`}
          value={onto}
          onChange={setChosenOnto}
          disabled={disabled}
        >
          <select
            id={`${formId}-onto`}
            name={`${formId}-onto`}
            value={onto}
            disabled={disabled}
            onChange={(event) => {
              setChosenOnto(event.target.value);
            }}
            className={CONTROL}
          >
            {heads.length === 0 && upstream.length === 0 && (
              <option value="">No ref to rebase onto</option>
            )}
            {heads.length > 0 && (
              <optgroup label="Integration heads">
                {heads.map((head) => (
                  <option key={head.name} value={head.name}>
                    {head.name}
                  </option>
                ))}
              </optgroup>
            )}
            {upstream.length > 0 && (
              <optgroup label="Upstream">
                {upstream.map((ref) => (
                  <option key={ref.name} value={ref.name}>
                    {ref.name}
                  </option>
                ))}
              </optgroup>
            )}
          </select>
        </FormField>
      </div>

      <p className="text-console-muted text-xs">{WORK_TREE_HINT}</p>

      <div className="flex items-center gap-3">
        <SubmitButton
          loading={form.loading}
          disabled={disabled || onto === ""}
        >
          Rebase
        </SubmitButton>
        {rebased !== null && (
          <span className="text-state-running font-mono text-xs">
            Rebased to {shortSha(rebased)}
          </span>
        )}
      </div>

      {workTreeNote !== null && workTreeNote !== undefined && (
        <Alert kind="warning">{workTreeNote}</Alert>
      )}
      {conflicts !== null && (
        <ConflictList paths={conflicts} message={conflictMessage} />
      )}
      {form.error !== null && (
        <Alert kind="error" onDismiss={form.clearError}>
          {form.error}
        </Alert>
      )}
    </form>
  );
}
