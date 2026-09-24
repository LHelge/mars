// Git operations for a project's session branches (`SPEC.md`, "User-facing
// features", Git operations, and "Git").
//
// One component in two places. On the project page's Branches tab it is the
// whole picture, in the order the operator works: the integration heads, the
// generic merge that integrates upstream — `origin/main` into `main`
// (`README.md`, "Operating notes") — and every session ref. In the session
// view the session table is filtered to one branch, so an operator finishing a
// session merges or pushes it without leaving the transcript.
//
// The orchestrator serialises git work per project, so while any form here is
// in flight every other one is disabled rather than queued behind it.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { listSessionBranches } from "../../services/git";
import { queryKeys } from "../../services/queryKeys";
import { projectQueries } from "../../services/queryOptions";
import type { Project, PushResult, SessionBranch } from "../../types";
import { Alert } from "../Alert";
import { EmptyState } from "../EmptyState";
import { LoadingState } from "../LoadingState";
import { SectionHeader } from "../SectionHeader";
import { Icon } from "../icons";
import { SubmitButton } from "../SubmitButton";
import { IntegrationHeadTable } from "./IntegrationHeadTable";
import { integrationHeads } from "./integrationHeads";
import { MergeForm } from "./MergeForm";
import { PushForm } from "./PushForm";
import { RebaseForm } from "./RebaseForm";
import { SessionBranchTable } from "./SessionBranchTable";
import { SYNC_TITLE } from "./syncHint";
import type { OpenRow, RowAction } from "./SessionBranchTable";

/**
 * The panel is either the project page's whole picture or one session's row,
 * and everything but the project belongs to the second shape: a `Sync` button
 * and a work-tree note have nothing to act on without a session. The props say
 * so, so the project page cannot pass a note nothing would read.
 */
export type GitActionsPanelProps = {
  project: Project;
} & (
  | {
      /** The table is this session's branch alone. */
      sessionId: string;
      /**
       * The session header's `Sync`, offered when the session has never been
       * synced and so has no ref in the mirror yet.
       */
      onSync?: () => void;
      syncing?: boolean;
      /** The reconciliation note of the session's latest `git` rebase event. */
      workTreeNote?: string | null;
    }
  | {
      sessionId?: never;
      onSync?: never;
      syncing?: never;
      workTreeNote?: never;
    }
);

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

  // Only for the session column's titles, and only on the project page, where
  // the list is the same cached read the Sessions tab polls; the session view
  // knows which session it is showing.
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
    setBusy((current) => (pending ? id : current === id ? null : current));
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

  const refreshButton = (
    <SubmitButton
      type="button"
      variant="ghost"
      loading={
        sessionBranches.isFetching ||
        (sessionId === undefined && branches.isFetching)
      }
      icon={Icon.refresh}
      onClick={() => {
        void sessionBranches.refetch();
        if (sessionId === undefined) {
          void branches.refetch();
        }
      }}
    >
      Refresh
    </SubmitButton>
  );

  // The session refs: every one on the project page, this session's alone in
  // the session view.
  const sessionTable = (
    <>
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
                icon={Icon.sync}
                onClick={onSync}
                title={SYNC_TITLE}
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
    </>
  );

  if (sessionId !== undefined) {
    return (
      <section className="space-y-3">
        <SectionHeader
          title="Branches"
          help="branches"
          description="This session's ref in the mirror, and what can be done with it."
          actions={refreshButton}
        />
        {sessionTable}
      </section>
    );
  }

  // The project page, in the order the operator works: what the integration
  // heads are, bringing upstream into them, then the session refs waiting to
  // be merged.
  const heads = integrationHeads(branches.data ?? [], project.default_branch);
  const defaultBranch = project.default_branch ?? "main";

  return (
    <section className="space-y-6">
      <SectionHeader
        title="Branches"
        help="branches"
        description="The mirror's integration heads, the merge that brings upstream into them, and every session's ref."
        actions={refreshButton}
      />

      <Block
        title="Integration heads"
        description="Mars's own branches. Sessions start from the default one, merges land in them, and a fetch never moves them."
      >
        {branches.isError && (
          <Alert kind="error">Could not load the branches of the mirror.</Alert>
        )}
        {branches.isPending ? (
          <LoadingState label="Loading integration heads" />
        ) : heads.length === 0 ? (
          branches.isSuccess && (
            <EmptyState
              title="No integration heads"
              description="The mirror has no branch of its own yet."
            />
          )
        ) : (
          <IntegrationHeadTable heads={heads} />
        )}
      </Block>

      <Block
        title="Merge any ref"
        description={
          <>
            Integrating upstream is a merge like any other:{" "}
            <span className="font-mono">origin/{defaultBranch}</span> into{" "}
            <span className="font-mono">{defaultBranch}</span>. Fetch first,
            so <span className="font-mono">origin/{defaultBranch}</span> is
            what the remote has now.
          </>
        }
      >
        <div className="border-console-border bg-console-surface rounded border px-3 py-3">
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
      </Block>

      <Block
        title="Session branches"
        description={`Session refs in the mirror, measured against ${project.default_branch ?? "the default branch"}.`}
      >
        {sessionTable}
      </Block>
    </section>
  );
}

/** One part of the project page's panel: a small heading over its content. */
function Block({
  title,
  description,
  children,
}: {
  title: string;
  description: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="space-y-2">
      <div>
        <h3 className="text-console-text text-sm font-semibold tracking-tight">
          {title}
        </h3>
        <p className="text-console-muted max-w-prose text-xs">{description}</p>
      </div>
      {children}
    </div>
  );
}
