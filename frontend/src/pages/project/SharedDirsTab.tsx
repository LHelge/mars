// The `shared-dirs` tab of `/projects/:id` (`SPEC.md`, "Shared directories"):
// the directories every session of this project mounts read-write, the form
// that adds one, and the two actions that are refused while the project has a
// live session.
//
// Emptying and removing are refused with 409 while any session is `running` or
// `creating`. The tab pre-disables both when the sessions tab has already read
// the list and it shows one — a courtesy, not a guard, because that list may
// be absent or a moment old. The server's 409 is the real answer and is shown
// on the row it was refused for.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import { Alert } from "../../components/Alert";
import { EmptyState } from "../../components/EmptyState";
import { LoadingState } from "../../components/LoadingState";
import { QueryErrorAlert } from "../../components/QueryErrorAlert";
import { SectionHeader } from "../../components/SectionHeader";
import { SubmitButton } from "../../components/SubmitButton";
import { queryKeys } from "../../services/queryKeys";
import {
  clearSharedDir,
  deleteSharedDir,
  listSharedDirs,
} from "../../services/projects";
import { errorMessage } from "../../services/errorMessage";
import type { Session, SharedDir } from "../../types";
import { formatDateTime, formatRelative } from "../../utils/format";
import { SharedDirForm } from "./SharedDirForm";
import type { ProjectTabPanelProps } from "./tabs";

/** `SPEC.md`, "Shared directories", in the one line the tab has room for. */
const MOUNT_HELP =
  "Mounted read-write into every session container of this project. The list is read at each launch; a session that is already running keeps the mounts it started with.";

/** The tooltip on an action the project's live sessions would have refused. */
const RUNNING_HINT = "A session is running";

const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";
const CELL = "py-1.5 pr-3 align-middle";

export function SharedDirsTab({ project }: ProjectTabPanelProps) {
  const queryClient = useQueryClient();

  const dirs = useQuery({
    queryKey: queryKeys.projects.sharedDirs(project.id),
    queryFn: () => listSharedDirs(project.id),
  });

  // Read from the cache, never fetched: the sessions tab owns that list, and
  // whether it has been opened only decides whether the buttons are disabled
  // before the request or after the 409. The key is matched by prefix because
  // the sessions list may be cached under a state filter.
  const live = queryClient
    .getQueriesData<Session[]>({
      queryKey: queryKeys.projects.sessions(project.id),
    })
    .some(([, sessions]) =>
      (sessions ?? []).some(
        (session) => session.state === "running" || session.state === "creating",
      ),
    );

  const rows = useMemo(
    () => [...(dirs.data ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [dirs.data],
  );

  return (
    <section className="space-y-3">
      <SectionHeader title="Shared directories" description={MOUNT_HELP} />

      {dirs.isError && (
        <QueryErrorAlert
          query={dirs}
          message={errorMessage(dirs.error)}
        />
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
          />
        )
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  Name
                </th>
                <th scope="col" className={HEAD}>
                  Container path
                </th>
                <th scope="col" className={`${HEAD} hidden md:table-cell`}>
                  Added
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  Actions
                </th>
              </tr>
            </thead>
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
  /** True when a cached sessions list shows a `running` or `creating` session. */
  live: boolean;
}

function SharedDirRow({ projectId, dir, live }: SharedDirRowProps) {
  const queryClient = useQueryClient();
  const listKey = queryKeys.projects.sharedDirs(projectId);

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
    if (
      !window.confirm(
        `Empty ${dir.name}? Everything under ${dir.container_path} is deleted; the next session starts it again from nothing.`,
      )
    ) {
      return;
    }
    remove.reset();
    clear.mutate();
  }

  function onRemove() {
    if (
      !window.confirm(
        `Remove ${dir.name}? Its contents are deleted and later sessions no longer mount ${dir.container_path}.`,
      )
    ) {
      return;
    }
    clear.reset();
    remove.mutate();
  }

  return (
    <>
      <tr className="border-console-border/60 border-b last:border-b-0">
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
          <div
            className="flex flex-wrap justify-end gap-1.5"
            title={live ? RUNNING_HINT : undefined}
          >
            <SubmitButton
              type="button"
              variant="ghost"
              loading={clear.isPending}
              disabled={busy || live}
              onClick={onClear}
            >
              Clear
            </SubmitButton>
            <SubmitButton
              type="button"
              variant="danger"
              loading={remove.isPending}
              disabled={busy || live}
              onClick={onRemove}
            >
              Remove
            </SubmitButton>
          </div>
        </td>
      </tr>

      {failure !== null && (
        <tr className="border-console-border/60 border-b last:border-b-0">
          <td colSpan={4} className="bg-console-surface/60 px-3 py-2">
            <Alert kind="error">{errorMessage(failure)}</Alert>
          </td>
        </tr>
      )}
    </>
  );
}
