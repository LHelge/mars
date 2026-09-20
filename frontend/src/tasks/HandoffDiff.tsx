// The diff of one hand-off's retained commit (`SPEC.md`, "Git": `GET
// /projects/{pid}/git/diff?handoff_id=`; `SPEC.md`, "Frontend", "Hand-off
// controls").
//
// A hand-off pins a commit, and the endpoint answers for that commit without
// fetching a live branch: the diff a reviewer sees here is the diff the
// approval would cover, however far the source session has run on since. That
// also makes the answer immutable, so it is fetched once and kept — nothing
// invalidates it and no refresh button is offered.
//
// It opens inside the drawer, under the hand-off section, rather than on a
// route of its own: the row it was opened from is the context.

import { useQuery } from "@tanstack/react-query";

import { Alert } from "../components/Alert";
import { DiffBody } from "../components/git/DiffBody";
import { LoadingState } from "../components/LoadingState";
import { getDiff } from "../services/git";
import { queryKeys } from "../services/queryKeys";
import type { Handoff } from "../types";
import { shortSha } from "../utils/format";
import { shortCommit } from "./launchRules";

export interface HandoffDiffProps {
  projectId: string;
  /** The hand-off whose retained commit is shown; never a live branch. */
  handoff: Handoff;
  onClose: () => void;
}

export function HandoffDiff({ projectId, handoff, onClose }: HandoffDiffProps) {
  const diff = useQuery({
    queryKey: queryKeys.projects.handoffDiff(projectId, handoff.id),
    queryFn: () => getDiff(projectId, { handoff_id: handoff.id }),
    // A retained commit never moves, so neither does its diff.
    staleTime: Infinity,
    retry: false,
  });

  return (
    <section
      aria-label={`Diff of hand-off ${shortCommit(handoff.commit)}`}
      className="border-console-border bg-console-bg space-y-3 rounded border p-3"
    >
      <header className="text-console-muted flex flex-wrap items-baseline gap-x-2 gap-y-1 font-mono text-xs">
        <span title={handoff.commit}>
          {diff.data?.base ?? "base"} → {shortCommit(handoff.commit)}
        </span>
        {diff.data && (
          <span title={diff.data.merge_base}>
            from {shortSha(diff.data.merge_base)}
          </span>
        )}
        <span className="text-console-text" title={handoff.source_branch}>
          {handoff.source_branch}
        </span>
        <button
          type="button"
          onClick={onClose}
          className="text-console-accent ml-auto text-xs underline underline-offset-2"
        >
          Close
        </button>
      </header>

      {diff.isError && <Alert kind="error">{diff.error.message}</Alert>}

      {diff.data === undefined ? (
        diff.isError ? null : (
          <LoadingState label="Loading the diff" />
        )
      ) : (
        <DiffBody diff={diff.data} />
      )}
    </section>
  );
}
