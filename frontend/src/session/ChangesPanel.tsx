// The `Changes` side panel of the session view (`SPEC.md`, "Frontend",
// "Changes panel"): the diff of the session's branch against its base, read
// from `GET /projects/{pid}/git/diff?head=<session id>`.
//
// The panel owns its refresh. It fetches when it is opened — an inactive tab
// is unmounted, so the query lives exactly as long as the panel is on screen —
// and again whenever a `git` event arrives, which the store records as
// `gitEventSeq`. Nothing polls: the endpoint's own fetch-back emits no event,
// so a refresh cannot trigger itself.
//
// The comparison runs from the merge base, so a session keeps showing its own
// work after the branch it started from has moved on.

import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";

import { Alert } from "../components/Alert";
import { Icon, ICON_CLASS } from "../components/icons";
import { LoadingState } from "../components/LoadingState";
import { DiffBody } from "../components/git/DiffBody";
import { getDiff } from "../services/git";
import { queryKeys } from "../services/queryKeys";
import { projectQueries } from "../services/queryOptions";
import { formatRelative, shortSha } from "../utils/format";
import { useSessionStore } from "./sessionStore";
import type { SessionPanelProps } from "./sidePanels";

export function ChangesPanel({ session }: SessionPanelProps) {
  const queryClient = useQueryClient();
  const projectId = session.project_id;
  /** `null` is "whatever the project's default branch is", chosen server-side. */
  const [base, setBase] = useState<string | null>(null);

  const project = useQuery(projectQueries.detail(projectId));
  const branches = useQuery(projectQueries.branches(projectId));

  const diff = useQuery({
    queryKey: queryKeys.projects.diff(projectId, session.id, base ?? undefined),
    queryFn: ({ signal }) =>
      getDiff(projectId, { head: session.id }, base ?? undefined, signal),
    // A refetch replaces the diff in place instead of blanking the panel.
    placeholderData: keepPreviousData,
  });

  const gitEventSeq = useSessionStore(session.id, (state) => state.gitEventSeq);
  // A `git` event means the branch may have moved — including a failed sync,
  // whose outcome the next fetch reports. The seq only ever grows, so the
  // last one seen is enough to tell a new event from a re-render.
  const seenGitSeq = useRef(gitEventSeq);
  useEffect(() => {
    if (seenGitSeq.current === gitEventSeq) {
      return;
    }
    seenGitSeq.current = gitEventSeq;
    void queryClient.invalidateQueries({
      queryKey: queryKeys.projects.diff(projectId, session.id, base ?? undefined),
    });
  }, [base, gitEventSeq, projectId, queryClient, session.id]);

  // Integration heads and upstream refs are both legal bases; session refs are
  // not, and the session's own branch is never something to compare against.
  const bases = (branches.data ?? []).filter(
    (branch) => branch.kind === "head" || branch.kind === "upstream",
  );
  const defaultBranch = project.data?.default_branch;

  return (
    <div className="space-y-3 p-3">
      <header className="space-y-2">
        <div className="text-console-muted flex flex-wrap items-baseline gap-x-2 gap-y-1 font-mono text-xs">
          <label htmlFor="changes-base" className="sr-only">
            Compare against
          </label>
          <select
            id="changes-base"
            value={base ?? ""}
            onChange={(event) => {
              setBase(event.target.value === "" ? null : event.target.value);
            }}
            className="border-console-border bg-console-bg text-console-text max-w-40 truncate rounded border px-1 py-0.5 font-mono text-xs"
          >
            <option value="">
              {defaultBranch === undefined
                ? "Default branch"
                : `${defaultBranch} (default)`}
            </option>
            {bases.map((branch) => (
              <option key={branch.name} value={branch.name}>
                {branch.name}
              </option>
            ))}
          </select>
          {/* The response carries the merge base as a commit and the head as
              its API name — the head's own commit is not part of `Diff` — so
              the branch stands in for a second short sha. */}
          {diff.data && (
            <span title={diff.data.merge_base}>
              {shortSha(diff.data.merge_base)} → {session.branch}
            </span>
          )}
          <button
            type="button"
            onClick={() => {
              void queryClient.invalidateQueries({
                queryKey: queryKeys.projects.diff(
                  projectId,
                  session.id,
                  base ?? undefined,
                ),
              });
            }}
            className="text-console-accent ml-auto flex items-center gap-1"
          >
            <Icon.refresh
              aria-hidden="true"
              className={`${ICON_CLASS} ${diff.isFetching ? "animate-spin" : ""}`}
            />
            Refresh
          </button>
        </div>
        {diff.data && !diff.isFetching && (
          <p className="text-console-muted font-mono text-xs">
            Read {formatRelative(new Date(diff.dataUpdatedAt).toISOString())}
          </p>
        )}
      </header>

      {diff.isError && (
        <Alert kind="error">
          <p>{diff.error.message}</p>
          <button
            type="button"
            onClick={() => void diff.refetch()}
            className="text-console-accent pt-1 text-xs underline underline-offset-2"
          >
            Retry
          </button>
        </Alert>
      )}

      {diff.data === undefined ? (
        diff.isError ? null : (
          <LoadingState label="Loading the diff" />
        )
      ) : (
        <DiffBody diff={diff.data} />
      )}
    </div>
  );
}
