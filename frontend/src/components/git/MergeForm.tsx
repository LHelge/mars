// Merging a ref into an integration head (`SPEC.md`, "Git": `POST
// /projects/{pid}/git/merge`).
//
// Two shapes of the same form: a session branch's row fixes the source and
// only asks where it goes, while the project page's generic form chooses both
// — that is how upstream is integrated, `origin/main` into `main`
// (`README.md`, "Operating notes"). The props say which shape this is, so a
// fixed source and a preselected one cannot be passed together.
//
// The task hand-off form of `MergeInput` (`task_id` + `handoff_id`) is the
// board's own action and not a mode of this form: this component only ever
// merges a branch into a head, and says so: a branch merge records no review
// approval (`SPEC.md`, "Git", `MergeInput`; `ARCHITECTURE.md`, "Git model").

import { useState } from "react";
import { merge } from "../../services/git";
import type { Branch } from "../../types";
import { shortSha } from "../../utils/format";
import { FieldShell } from "../FieldShell";
import { FIELD } from "../fieldStyles";
import { FormField } from "../FormField";
import { HelpLink } from "../HelpLink";
import { Icon } from "../icons";
import { SubmitButton } from "../SubmitButton";
import {
  chosenOr,
  keptIfKnown,
  refsOfKind,
  useGitAction,
  useReportBusy,
} from "./formState";
import type { ReportBusy } from "./formState";
import { GitFormShell, GitResultNote } from "./GitFormShell";
import { ReadOnlyField } from "./ReadOnlyField";
import { RefOptions } from "./RefOptions";

/**
 * Which of the two forms this is. A row's merge has a fixed source and may
 * label it, because the source is an opaque session id; the generic merge
 * chooses one and may start from a preselection. Neither set is meaningful in
 * the other's shape, so the props are one or the other.
 */
type MergeSourceProps =
  | { source: string; sourceLabel?: string; defaultSource?: never }
  | { source?: never; sourceLabel?: never; defaultSource?: string };

export type MergeFormProps = {
  projectId: string;
  branches: Branch[];
  /** Preselected target; the project's default branch. */
  defaultTarget?: string;
  /** Distinguishes this form's controls from every other one on the page. */
  formId: string;
  disabled: boolean;
  onBusy: ReportBusy;
  /** A merge moved an integration head: everything git is now stale. */
  onMerged: () => void;
} & MergeSourceProps;

export function MergeForm({
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

  const from = source ?? keptIfKnown(chosenSource, branches);
  const target = chosenOr(chosenTarget, heads);

  const action = useGitAction(async () => {
    const result = await merge(projectId, {
      target,
      source: from,
      ...(message.trim() === "" ? {} : { message: message.trim() }),
    });
    onMerged();
    return result.commit;
  });

  const ready = from !== "" && target !== "";

  useReportBusy(formId, action.loading, onBusy);

  return (
    <GitFormShell action={action}>
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
                <RefOptions
                  branches={branches}
                  kind="upstream"
                  label="Upstream"
                />
                <RefOptions
                  branches={branches}
                  kind="head"
                  label="Integration heads"
                />
                <RefOptions
                  branches={branches}
                  kind="session"
                  label="Session branches"
                />
              </select>
            )}
          </FieldShell>
        ) : (
          <ReadOnlyField label="Source" value={sourceLabel ?? source} />
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

      <p className="text-console-muted text-xs">
        A branch merge grants no task approval; reviewed task work is merged
        from the task&apos;s own merge action. <HelpLink topic="branches" />
      </p>

      <div className="flex items-center gap-3">
        <SubmitButton
          loading={action.loading}
          disabled={disabled || !ready}
          icon={Icon.merge}
        >
          Merge
        </SubmitButton>
        {action.result !== null && (
          <GitResultNote>Merged at {shortSha(action.result)}</GitResultNote>
        )}
      </div>
    </GitFormShell>
  );
}
