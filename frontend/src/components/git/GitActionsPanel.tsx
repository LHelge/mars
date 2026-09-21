// Git operations for a project's session branches (`SPEC.md`, "User-facing
// features", Git operations, and "Git").
//
// One component in two places. On the project page it is the whole picture:
// every session ref, plus the generic merge that integrates upstream —
// `origin/main` into `main` (`README.md`, "Operating notes"). In the session
// view the same table is filtered to one branch, so an operator finishing a
// session merges or pushes it without leaving the transcript.
//
// The orchestrator serialises git work per project, so while any form here is
// in flight every other one is disabled rather than queued behind it.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo, useState } from "react";

import { listSessionBranches } from "../../services/git";
import { queryKeys } from "../../services/queryKeys";
import { projectQueries } from "../../services/queryOptions";
import type { Project, PushResult, SessionBranch } from "../../types";
import { Alert } from "../Alert";
import { EmptyState } from "../EmptyState";
import { LoadingState } from "../LoadingState";
import { SectionHeader } from "../SectionHeader";
import { SubmitButton } from "../SubmitButton";
import { MergeForm } from "./MergeForm";
import { PushForm } from "./PushForm";
import { RebaseForm } from "./RebaseForm";
import { SessionBranchTable } from "./SessionBranchTable";
import type { OpenRow, RowAction } from "./SessionBranchTable";

export interface GitActionsPanelProps {
  project: Project;
  /** Set in the session view: the table is that session's branch alone. */
  sessionId?: string;
  /**
   * The session header's `Sync`, offered when the session has never been
   * synced and so has no ref in the mirror yet.
   */
  onSync?: () => void;
  syncing?: boolean;
  /** The reconciliation note of the session's latest `git` rebase event. */
  workTreeNote?: string | null;
}

export function GitActionsPanel({
  project,
  sessionId,
  onSync,
  syncing = false,
  workTreeNote,
}: GitActionsPanelProps) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState<OpenRow | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  /** The compare link each row kept from its last push, by session id. */
  const [compareLinks, setCompareLinks] = useState<
    Record<string, { commit: string; url: string }>
  >({});

  const sessionBranches = useQuery({
    queryKey: queryKeys.projects.sessionBranches(project.id),
    queryFn: () => listSessionBranches(project.id),
    refetchOnWindowFocus: true,
  });

  const branches = useQuery(projectQueries.branches(project.id));

  // Only for the session column's titles, and only where the project page has
  // the list anyway; the session view knows which session it is showing.
  const sessions = useQuery({
    ...projectQueries.sessions(project.id),
    enabled: sessionId === undefined,
  });

  const titles = useMemo(
    () =>
      new Map<string, string>(
        (sessions.data ?? []).flatMap((session) =>
          session.title === null ? [] : [[session.id, session.title]],
        ),
      ),
    [sessions.data],
  );

  const rows = useMemo(() => {
    const all = sessionBranches.data ?? [];
    const mine =
      sessionId === undefined
        ? all
        : all.filter((row) => row.session_id === sessionId);
    return [...mine].sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  }, [sessionBranches.data, sessionId]);

  /** Everything under `["projects", id]`: the refs, the counts, the project. */
  const refresh = useCallback(() => {
    void queryClient.invalidateQueries({
      queryKey: queryKeys.projects.detail(project.id),
    });
  }, [queryClient, project.id]);

  const report = useCallback((id: string, pending: boolean) => {
    setBusy((current) =>
      pending ? id : current === id ? null : current,
    );
  }, []);

  const onPushed = useCallback(
    (row: SessionBranch, result: PushResult, url: string | null) => {
      setCompareLinks((current) => {
        const next = { ...current };
        if (url === null) {
          delete next[row.session_id];
        } else {
          next[row.session_id] = { commit: result.commit, url };
        }
        return next;
      });
      refresh();
    },
    [refresh],
  );

  const toggle = useCallback((id: string, action: RowAction) => {
    setOpen((current) =>
      current?.sessionId === id && current.action === action
        ? null
        : { sessionId: id, action },
    );
  }, []);

  const disabledFor = (formId: string): boolean =>
    busy !== null && busy !== formId;

  const renderForm = (row: SessionBranch) => {
    if (open === null || open.sessionId !== row.session_id) {
      return null;
    }
    const label = titles.get(row.session_id) ?? row.ref;
    const formId = `git-${open.action}-${row.session_id}`;

    switch (open.action) {
      case "merge":
        return (
          <MergeForm
            projectId={project.id}
            branches={branches.data ?? []}
            source={row.session_id}
            sourceLabel={label}
            defaultTarget={project.default_branch ?? undefined}
            formId={formId}
            disabled={disabledFor(formId)}
            onBusy={report}
            onMerged={refresh}
          />
        );
      case "rebase":
        return (
          <RebaseForm
            projectId={project.id}
            branches={branches.data ?? []}
            branch={row.session_id}
            branchLabel={label}
            defaultOnto={project.default_branch ?? undefined}
            formId={formId}
            disabled={disabledFor(formId)}
            onBusy={report}
            onRebased={refresh}
            workTreeNote={workTreeNote}
          />
        );
      case "push":
        return (
          <PushForm
            projectId={project.id}
            gitRef={row.session_id}
            isSession
            refLabel={label}
            remoteUrl={project.remote_url}
            compareTarget={project.default_branch}
            formId={formId}
            disabled={disabledFor(formId)}
            onBusy={report}
            onPushed={(result, url) => {
              onPushed(row, result, url);
            }}
          />
        );
    }
  };

  const genericId = "git-merge-generic";

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Branches"
        description={
          sessionId === undefined
            ? `Session refs in the mirror, measured against ${project.default_branch ?? "the default branch"}.`
            : "This session's ref in the mirror, and what can be done with it."
        }
        actions={
          <SubmitButton
            type="button"
            variant="ghost"
            loading={sessionBranches.isFetching}
            onClick={() => {
              void sessionBranches.refetch();
            }}
          >
            Refresh
          </SubmitButton>
        }
      />

      {sessionBranches.isError && (
        <Alert kind="error">Could not load the session branches.</Alert>
      )}

      {sessionBranches.isPending ? (
        <LoadingState label="Loading session branches" />
      ) : rows.length === 0 ? (
        sessionId === undefined ? (
          <EmptyState
            title="No session branches yet"
            description="A session's work appears here once it has been synced into the mirror."
          />
        ) : (
          <div className="flex flex-wrap items-center gap-3">
            <p className="text-console-muted text-sm">
              not synced yet — this session has no ref in the mirror.
            </p>
            {onSync !== undefined && (
              <SubmitButton
                type="button"
                variant="ghost"
                loading={syncing}
                onClick={onSync}
              >
                Sync
              </SubmitButton>
            )}
          </div>
        )
      ) : (
        <SessionBranchTable
          rows={rows}
          titles={titles}
          compareLinks={compareLinks}
          open={open}
          onToggle={toggle}
          renderForm={renderForm}
          disabled={busy !== null}
        />
      )}

      {sessionId === undefined && (
        <div className="border-console-border bg-console-surface space-y-3 rounded border px-3 py-3">
          <h3 className="text-console-text text-sm font-semibold tracking-tight">
            Merge any ref
          </h3>
          <p className="text-console-muted text-xs">
            Integrating upstream is a merge like any other:{" "}
            <span className="font-mono">
              origin/{project.default_branch ?? "main"}
            </span>{" "}
            into{" "}
            <span className="font-mono">
              {project.default_branch ?? "main"}
            </span>
            .
          </p>
          <MergeForm
            projectId={project.id}
            branches={branches.data ?? []}
            defaultSource={
              project.default_branch === null
                ? undefined
                : `origin/${project.default_branch}`
            }
            defaultTarget={project.default_branch ?? undefined}
            formId={genericId}
            disabled={disabledFor(genericId)}
            onBusy={report}
            onMerged={refresh}
          />
        </div>
      )}
    </section>
  );
}
