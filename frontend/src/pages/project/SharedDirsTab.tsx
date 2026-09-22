// The `shared-dirs` tab of `/projects/:id` (`SPEC.md`, "Shared directories"):
// the directories every session of this project mounts read-write, the form
// that adds one, and the two actions that are refused while the project has a
// live session.
//
// Emptying and removing are refused with 409 while any session is `running` or
// `creating`, so the tab disables both while this project has one. A disabled
// button is a guard and not a courtesy, so it has to be able to come back: the
// tab subscribes to the project's session list itself and polls it, rather than
// reading whatever the sessions tab happened to leave in the cache before it
// was unmounted. Until that read answers nothing is disabled — the server's 409
// is the authority either way, and it is shown on the row it was refused for.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Alert } from "../../components/Alert";
import { EmptyState } from "../../components/EmptyState";
import { LoadingState } from "../../components/LoadingState";
import { QueryErrorAlert } from "../../components/QueryErrorAlert";
import { SectionHeader } from "../../components/SectionHeader";
import { ConfirmPanel } from "../../components/ConfirmPanel";
import { SubmitButton } from "../../components/SubmitButton";
import { TableHead } from "../../components/TableHead";
import {
  CELL,
  ROW,
  SPAN_CELL,
  TABLE,
  X_SCROLLER,
  type TableColumn,
} from "../../components/tableStyles";
import { queryKeys } from "../../services/queryKeys";
import { projectQueries } from "../../services/queryOptions";
import {
  clearSharedDir,
  deleteSharedDir,
  listSharedDirs,
} from "../../services/projects";
import { errorMessage } from "../../services/errorMessage";
import type { SharedDir } from "../../types";
import { formatDateTime, formatRelative } from "../../utils/format";
import { SharedDirForm } from "./SharedDirForm";
import type { ProjectTabPanelProps } from "./tabs";

/**
 * `SPEC.md`, "Shared directories", and `README.md`, "Operating notes", in
 * the few lines the tab has room for: why share, and what not to.
 */
const MOUNT_HELP =
  "Mounted read-write into every session container of this project, so a download cache, or Cargo's build directory, is kept once instead of once per session. Share what tolerates several sessions writing at once; keep per session what a branch rewrites in place, such as node_modules, a virtualenv or most build output. The list is read at each launch; a running session keeps the mounts it started with.";

/** The banner's lead-in while the project's live sessions would refuse both actions. */
const RUNNING_HINT = "A session is running";

/** The same reason, said on each row beside the two buttons it disables. */
const ROW_BLOCKED = "Blocked while a session of this project is live.";

/**
 * How often the session list behind the two disabled buttons is re-read. A
 * session ending is what re-enables them, and nothing else on this tab is
 * watching for it, so the poll is this tab's own and stops with it.
 */
const SESSIONS_POLL_MS = 15_000;

const COLUMNS: readonly TableColumn[] = [
  { label: "Name" },
  { label: "Container path" },
  { label: "Added", className: "hidden md:table-cell" },
  { label: "Actions", className: "pr-0 text-right" },
];

export function SharedDirsTab({ project }: ProjectTabPanelProps) {
  const dirs = useQuery({
    queryKey: queryKeys.projects.sharedDirs(project.id),
    queryFn: () => listSharedDirs(project.id),
  });

  // The unfiltered list, as `projectQueries.sessions` spells it, with the poll
  // this tab needs of it; a hidden tab is not polled.
  const sessions = useQuery({
    ...projectQueries.sessions(project.id),
    refetchInterval: SESSIONS_POLL_MS,
    refetchIntervalInBackground: false,
  });

  // Only a list that arrived may disable anything: a pending or failed read
  // leaves the 409 as the only answer (`SPEC.md`, "Frontend", Read failures).
  const live = (sessions.data ?? []).some(
    (session) => session.state === "running" || session.state === "creating",
  );

  const rows = useMemo(
    () => [...(dirs.data ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [dirs.data],
  );

  return (
    <section className="space-y-3">
      <SectionHeader
        title="Shared directories"
        description={MOUNT_HELP}
        help="shared-directories"
      />

      {dirs.isError && (
        <QueryErrorAlert query={dirs} message={errorMessage(dirs.error)} />
      )}

      {live && (
        <Alert kind="warning">
          {RUNNING_HINT}: emptying and removing wait until this project has no
          session that is running or being created.
        </Alert>
      )}

      <SharedDirForm projectId={project.id} />

      {/* A failed read never reads as "none": only a successful one knows
          (`SPEC.md`, "Frontend", Read failures). */}
      {dirs.isPending ? (
        <LoadingState label="Loading shared directories" />
      ) : rows.length === 0 ? (
        dirs.isSuccess && (
          <EmptyState
            title="No shared directories yet"
            description="Add one above, or pick a starting point for the ecosystem this project builds with."
            help="shared-directories"
          />
        )
      ) : (
        <div className={X_SCROLLER}>
          <table className={TABLE}>
            <TableHead columns={COLUMNS} />
            <tbody>
              {rows.map((dir) => (
                <SharedDirRow
                  key={dir.name}
                  projectId={project.id}
                  dir={dir}
                  live={live}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

interface SharedDirRowProps {
  projectId: string;
  dir: SharedDir;
  /** True when the polled session list shows a `running` or `creating` one. */
  live: boolean;
}

function SharedDirRow({ projectId, dir, live }: SharedDirRowProps) {
  const queryClient = useQueryClient();
  const listKey = queryKeys.projects.sharedDirs(projectId);
  /** Which of the two destructive actions is waiting to be confirmed. */
  const [confirming, setConfirming] = useState<"clear" | "remove" | null>(null);
  const blockedId = `shared-dir-${dir.name}-blocked`;

  const clear = useMutation({
    mutationFn: () => clearSharedDir(projectId, dir.name),
    onSuccess: () => {
      // Nothing on screen changes, but the row's timestamps come from the
      // list, so it is read again rather than assumed.
      void queryClient.invalidateQueries({ queryKey: listKey });
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteSharedDir(projectId, dir.name),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: listKey });
    },
  });

  const busy = clear.isPending || remove.isPending;
  const failure = remove.error ?? clear.error;

  function onClear() {
    setConfirming(null);
    remove.reset();
    clear.mutate();
  }

  function onRemove() {
    setConfirming(null);
    clear.reset();
    remove.mutate();
  }

  return (
    <>
      <tr className={ROW}>
        <td className={`${CELL} text-console-text font-mono text-xs`}>
          {dir.name}
        </td>

        <td className={`${CELL} text-console-text font-mono text-xs`}>
          {dir.container_path}
        </td>

        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap md:table-cell`}
          title={formatDateTime(dir.created_at)}
        >
          {formatRelative(dir.created_at)}
        </td>

        <td className={`${CELL} pr-0`}>
          <div className="flex flex-wrap justify-end gap-1.5">
            <SubmitButton
              type="button"
              variant="ghost"
              loading={clear.isPending}
              disabled={busy || live}
              aria-describedby={live ? blockedId : undefined}
              onClick={() => {
                setConfirming("clear");
              }}
            >
              Clear
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="danger"
              loading={remove.isPending}
              disabled={busy || live}
              aria-describedby={live ? blockedId : undefined}
              onClick={() => {
                setConfirming("remove");
              }}
            >
              Remove
            </SubmitButton>
          </div>
          {live && (
            <p
              id={blockedId}
              className="text-console-muted pt-1 text-right text-xs"
            >
              {ROW_BLOCKED}
            </p>
          )}
        </td>
      </tr>

      {confirming !== null && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className={SPAN_CELL}>
            {confirming === "clear" ? (
              <ConfirmPanel
                message={`Empty ${dir.name}? Everything under ${dir.container_path} is deleted; the next session starts it again from nothing.`}
                confirmLabel={`Empty ${dir.name}`}
                pending={clear.isPending}
                onConfirm={onClear}
                onCancel={() => {
                  setConfirming(null);
                }}
              />
            ) : (
              <ConfirmPanel
                message={`Remove ${dir.name}? Its contents are deleted and later sessions no longer mount ${dir.container_path}.`}
                confirmLabel={`Remove ${dir.name}`}
                pending={remove.isPending}
                onConfirm={onRemove}
                onCancel={() => {
                  setConfirming(null);
                }}
              />
            )}
          </td>
        </tr>
      )}

      {failure !== null && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className={SPAN_CELL}>
            <Alert kind="error">{errorMessage(failure)}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}
