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
import { ResolveWithAgent } from "./ResolveWithAgent";
import { commitOfSource, sourceName, sourceOfRef } from "./resolveMessage";
import type { ResolveMerge, ResolveSource } from "./resolveMessage";

/**
 * Which of the two forms this is. A row's merge has a fixed source and may
 * label it, because the source is an opaque session id; the generic merge
 * chooses one and may start from a preselection. Neither set is meaningful in
 * the other's shape, so the props are one or the other.
 */
type MergeSourceProps =
  | {
      source: string;
      sourceLabel?: string;
      /** The row's commit, which a conflict's resolver is told to expect. */
      sourceCommit?: string;
      defaultSource?: never;
    }
  | {
      source?: never;
      sourceLabel?: never;
      sourceCommit?: never;
      defaultSource?: string;
    };

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
  sourceCommit,
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
  // The source and target the last attempt sent, so a conflict is resolved
  // for the merge that conflicted and not for whatever the selects say now.
  const [attempt, setAttempt] = useState<{
    source: string;
    target: string;
  } | null>(null);

  const from = source ?? keptIfKnown(chosenSource, branches);
  const target = chosenOr(chosenTarget, heads);

  const action = useGitAction(async () => {
    setAttempt({ source: from, target });
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

  const conflict = action.conflict;
  const resolve: ResolveMerge | null =
    conflict === null || attempt === null
      ? null
      : resolveMergeOf(attempt, conflict.paths, branches, source, sourceCommit);

  return (
    <>
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
      {resolve !== null && (
        // Outside the merge's own `<form>`: the resolver launch is a form of its
        // own, and forms do not nest. Keyed by the merge, so another conflict
        // opens with its own generated message.
        <ResolveWithAgent
          key={[
            resolve.target,
            sourceName(resolve.source),
            ...resolve.paths,
          ].join("\n")}
          projectId={projectId}
          merge={resolve}
          disabled={disabled}
        />
      )}
    </>
  );
}

/**
 * The conflicting merge as the resolver is told about it: a row's fixed source
 * is a session, a chosen one is read from the listing, and each commit is the
 * one the panel already has loaded, or none.
 */
function resolveMergeOf(
  attempt: { source: string; target: string },
  paths: string[],
  branches: Branch[],
  rowSession: string | undefined,
  rowCommit: string | undefined,
): ResolveMerge {
  const source: ResolveSource =
    rowSession === undefined
      ? sourceOfRef(attempt.source, branches)
      : { kind: "session", sessionId: rowSession };
  return {
    source,
    sourceCommit: rowCommit ?? commitOfSource(source, branches),
    target: attempt.target,
    targetCommit:
      branches.find(
        (branch) => branch.kind === "head" && branch.name === attempt.target,
      )?.commit ?? null,
    paths,
  };
}
