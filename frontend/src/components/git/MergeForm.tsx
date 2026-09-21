// Merging a ref into an integration head (`SPEC.md`, "Git": `POST
// /projects/{pid}/git/merge`).
//
// Two shapes of the same form: a session branch's row fixes the source and
// only asks where it goes, while the project page's generic form chooses both
// — that is how upstream is integrated, `origin/main` into `main`
// (`README.md`, "Operating notes").
//
// The task hand-off form of `MergeInput` (`task_id` + `handoff_id`) is the
// board's, and `mode` is here so the board can reuse this component without
// reshaping it; only `branch` is implemented.

import { useState } from "react";
import { useFormSubmit } from "../../hooks/useFormSubmit";
import { isGitConflict, merge } from "../../services/git";
import type { Branch } from "../../types";
import { shortSha } from "../../utils/format";
import { Alert } from "../Alert";
import { FieldShell } from "../FieldShell";
import { FIELD } from "../fieldStyles";
import { FormField } from "../FormField";
import { SubmitButton } from "../SubmitButton";
import { ConflictList } from "./ConflictList";
import { chosenOr, keptIfKnown, refsOfKind, useReportBusy } from "./formState";
import type { ReportBusy } from "./formState";

export interface MergeFormProps {
  /** Only `branch` is implemented; the board's drawer will add `task`. */
  mode?: "branch";
  projectId: string;
  branches: Branch[];
  /** A fixed source — a row's session id — or `undefined` to choose one. */
  source?: string;
  /** The source's name, when the fixed source is an opaque session id. */
  sourceLabel?: string;
  /** Preselected source of the generic form. */
  defaultSource?: string;
  /** Preselected target; the project's default branch. */
  defaultTarget?: string;
  /** Distinguishes this form's controls from every other one on the page. */
  formId: string;
  disabled: boolean;
  onBusy: ReportBusy;
  /** A merge moved an integration head: everything git is now stale. */
  onMerged: () => void;
}

export function MergeForm({
  mode = "branch",
  projectId,
  branches,
  source,
  sourceLabel,
  defaultSource,
  defaultTarget,
  formId,
  disabled,
  onBusy,
  onMerged,
}: MergeFormProps) {
  const heads = refsOfKind(branches, "head");
  const [chosenSource, setChosenSource] = useState(defaultSource ?? "");
  const [chosenTarget, setChosenTarget] = useState(defaultTarget ?? "");
  const [message, setMessage] = useState("");
  const [merged, setMerged] = useState<string | null>(null);
  const [conflicts, setConflicts] = useState<string[] | null>(null);
  const [conflictMessage, setConflictMessage] = useState("");

  const from = source ?? keptIfKnown(chosenSource, branches);
  const target = chosenOr(chosenTarget, heads);

  const form = useFormSubmit(async () => {
    setMerged(null);
    setConflicts(null);
    try {
      const result = await merge(projectId, {
        target,
        source: from,
        ...(message.trim() === "" ? {} : { message: message.trim() }),
      });
      setMerged(result.commit);
      onMerged();
    } catch (caught) {
      if (isGitConflict(caught)) {
        setConflicts(caught.conflicts);
        setConflictMessage(caught.error);
        return;
      }
      throw caught;
    }
  });

  const ready = mode === "branch" && from !== "" && target !== "";

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
        {source === undefined ? (
          <FieldShell label="Source" name={`${formId}-source`}>
            {(control) => (
              <select
                {...control}
                value={from}
                disabled={disabled}
                onChange={(event) => {
                  setChosenSource(event.target.value);
                }}
                className={FIELD}
              >
                <option value="">Choose a ref…</option>
                <Options branches={branches} kind="upstream" label="Upstream" />
                <Options
                  branches={branches}
                  kind="head"
                  label="Integration heads"
                />
                <Options
                  branches={branches}
                  kind="session"
                  label="Session branches"
                />
              </select>
            )}
          </FieldShell>
        ) : (
          <div className="flex flex-col gap-1.5">
            <span className="text-console-muted text-xs">Source</span>
            <p className="text-console-text truncate py-1.5 font-mono text-sm">
              {sourceLabel ?? source}
            </p>
          </div>
        )}

        <FieldShell label="Target" name={`${formId}-target`}>
          {(control) => (
            <select
              {...control}
              value={target}
              disabled={disabled}
              onChange={(event) => {
                setChosenTarget(event.target.value);
              }}
              className={FIELD}
            >
              {heads.length === 0 && (
                <option value="">No integration head</option>
              )}
              {heads.map((head) => (
                <option key={head.name} value={head.name}>
                  {head.name}
                </option>
              ))}
            </select>
          )}
        </FieldShell>
      </div>

      <FormField
        label="Message"
        name={`${formId}-message`}
        value={message}
        onChange={setMessage}
        disabled={disabled}
        hint="Left empty, git writes its own merge message."
      />

      <div className="flex items-center gap-3">
        <SubmitButton loading={form.loading} disabled={disabled || !ready}>
          Merge
        </SubmitButton>
        {merged !== null && (
          <span className="text-state-running font-mono text-xs">
            Merged at {shortSha(merged)}
          </span>
        )}
      </div>

      {conflicts !== null && (
        <ConflictList paths={conflicts} message={conflictMessage} />
      )}
      {form.error !== null && (
        <Alert kind="error" onDismiss={form.reset}>
          {form.error}
        </Alert>
      )}
    </form>
  );
}

function Options({
  branches,
  kind,
  label,
}: {
  branches: Branch[];
  kind: Branch["kind"];
  label: string;
}) {
  const refs = refsOfKind(branches, kind);
  if (refs.length === 0) {
    return null;
  }
  return (
    <optgroup label={label}>
      {refs.map((branch) => (
        <option key={branch.name} value={branch.name}>
          {branch.name}
        </option>
      ))}
    </optgroup>
  );
}

