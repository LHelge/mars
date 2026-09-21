// The `sessions` tab of `/projects/:id` (`SPEC.md`, "Frontend", Routes):
// `GET /projects/{pid}/sessions` as one row per session, with the launch form
// above it.
//
// A session list is the operator's working set for the project, so it is a
// table and not cards: state first, because that is what is being scanned for,
// then what the session is, and the numbers last and right-aligned so a column
// of costs lines up. Sessions keep running when nobody watches them
// (`SPEC.md`, "User-facing features"), so the list polls — quickly while
// anything is still `creating` or `running`, slowly once the project has gone
// quiet — and stops entirely while the browser tab is in the background. The
// state filter narrows what has already arrived rather than asking the server
// again, so switching it is instant and never shows one state's rows as
// another's.
//
// Deleting is only offered for a session that has finished; the API refuses
// any other (`DELETE /sessions/{id}`: must be `done` or `failed`), and the
// refusal is shown on the row it belongs to rather than at the top of the page.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Fragment, useMemo, useState } from "react";
import { Link } from "react-router";
import { EmptyState } from "../../components/EmptyState";
import { LoadingState } from "../../components/LoadingState";
import { QueryErrorAlert } from "../../components/QueryErrorAlert";
import { ConfirmPanel } from "../../components/ConfirmPanel";
import { SessionStatePill } from "../../components/SessionStatePill";
import { TableHead } from "../../components/TableHead";
import {
  CELL,
  ROW,
  SPAN_CELL,
  TABLE,
  type TableColumn,
} from "../../components/tableStyles";
import { GitActionsPanel } from "../../components/git/GitActionsPanel";
import { errorMessage, logUnexpected } from "../../services/errorMessage";
import { LaunchSourceTag } from "../../session/LaunchSourceTag";
import { queryKeys } from "../../services/queryKeys";
import { projectQueries } from "../../services/queryOptions";
import { deleteSession } from "../../services/sessions";
import type { Session, SessionState } from "../../types";
import {
  COST_DECIMALS,
  formatRelative,
  formatUsd,
  PLACEHOLDER,
  shortId,
} from "../../utils/format";
import { LaunchSessionForm } from "./LaunchSessionForm";
import type { ProjectTabPanelProps } from "./tabs";

/** While a session is still starting or working, its row changes on its own. */
const BUSY_POLL_MS = 10_000;
/** Once everything has settled, the list only has to stay roughly current. */
const IDLE_POLL_MS = 60_000;

type Filter = SessionState | "all";

const FILTERS: { value: Filter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "parked", label: "Parked" },
  { value: "done", label: "Done" },
  { value: "failed", label: "Failed" },
];

const COLUMNS: readonly TableColumn[] = [
  { label: "State" },
  { label: "Session" },
  { label: "Kind", className: "hidden sm:table-cell" },
  { label: "Profile", className: "hidden md:table-cell" },
  { label: "Branch", className: "hidden lg:table-cell" },
  { label: "Activity", className: "hidden sm:table-cell" },
  { label: "Cost", className: "text-right" },
  { label: "Actions", className: "pr-0 text-right", srOnly: true },
];

function isBusy(session: Session): boolean {
  return session.state === "creating" || session.state === "running";
}

/**
 * How a confirmation names the session it is about: the row's own title where
 * there is one, its short id otherwise. An untitled session under a `Delete`
 * that said nothing else was the accessible-name problem this replaced.
 */
function sessionLabel(session: Session): string {
  return session.title ?? `session ${shortId(session.id, 8)}`;
}

export function SessionsTab({ project }: ProjectTabPanelProps) {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<Filter>("all");
  // The row whose `Delete` has been pressed once; pressing again confirms.
  const [confirming, setConfirming] = useState<string | null>(null);
  const [rowError, setRowError] = useState<{
    id: string;
    message: string;
  } | null>(null);

  // The whole list, always: the filter is a view of it and not a second read.
  // A per-state key would mean a request per filter and, between the click and
  // its answer, the previous filter's rows sitting under the new filter's
  // heading — `Failed` briefly listing running sessions. The panel below reads
  // the same unfiltered key, so this is also one polled request and not two.
  const sessions = useQuery({
    ...projectQueries.sessions(project.id),
    refetchIntervalInBackground: false,
    refetchInterval: (query) =>
      (query.state.data ?? []).some(isBusy) ? BUSY_POLL_MS : IDLE_POLL_MS,
  });

  // Only for the profile column; the launch form reads the same cached list.
  const profiles = useQuery(projectQueries.profiles(project.id));

  const profileNames = useMemo(
    () =>
      new Map<string, string>(
        (profiles.data ?? []).map((profile) => [profile.id, profile.name]),
      ),
    [profiles.data],
  );

  const rows = useMemo(
    () =>
      (sessions.data ?? [])
        .filter((session) => filter === "all" || session.state === filter)
        .sort((a, b) => b.created_at.localeCompare(a.created_at)),
    [sessions.data, filter],
  );

  const remove = useMutation({
    mutationFn: (id: string) => deleteSession(id),
    onSuccess: () => {
      setConfirming(null);
      setRowError(null);
      // The unfiltered key, which every filtered list extends.
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.sessions(project.id),
      });
    },
    onError: (caught: unknown, id: string) => {
      setConfirming(null);
      logUnexpected(caught);
      setRowError({
        id,
        message: errorMessage(caught, "Could not delete the session"),
      });
    },
  });

  return (
    <div className="space-y-6">
      <LaunchSessionForm project={project} />

      <section className="space-y-3">
        <div className="border-console-border flex flex-wrap items-center justify-between gap-2 border-b pb-2">
          <h2 className="text-console-text text-sm font-semibold tracking-tight">
            Sessions
          </h2>
          <div role="group" aria-label="Filter by state" className="flex gap-1">
            {FILTERS.map((entry) => (
              <button
                key={entry.value}
                type="button"
                aria-pressed={filter === entry.value}
                onClick={() => {
                  setFilter(entry.value);
                }}
                className={`rounded border px-2 py-0.5 font-mono text-xs ${
                  filter === entry.value
                    ? "border-console-accent text-console-text"
                    : "border-console-border text-console-muted hover:text-console-text"
                }`}
              >
                {entry.label}
              </button>
            ))}
          </div>
        </div>

        {sessions.isError && (
          <QueryErrorAlert
            query={sessions}
            message="Could not load the sessions of this project."
          />
        )}

        {/* Only a successful read may claim the project has no sessions
            (`SPEC.md`, "Frontend", Read failures). */}
        {sessions.isPending ? (
          <LoadingState label="Loading sessions" />
        ) : rows.length === 0 ? (
          sessions.isSuccess && (
            <EmptyState
              title={
                filter === "all"
                  ? "No sessions yet"
                  : `No ${filter} sessions right now`
              }
              description={
                filter === "all"
                  ? "Launch one above to put an agent on this project."
                  : "Choose another state to see the rest."
              }
            />
          )
        ) : (
          <table className={TABLE}>
            <TableHead columns={COLUMNS} />
            <tbody>
              {rows.map((session) => {
                const finished =
                  session.state === "done" || session.state === "failed";
                const error =
                  rowError?.id === session.id ? rowError.message : null;

                return (
                  <Fragment key={session.id}>
                    <tr className={ROW}>
                      <td className={CELL}>
                        <SessionStatePill
                          state={session.state}
                          error={session.error}
                        />
                      </td>
                      <td className={`${CELL} min-w-0`}>
                        {/* Who launched it, where its name is: a row with no
                            tag was launched by a person (`SPEC.md`,
                            "Sessions"). */}
                        <span className="flex flex-wrap items-center gap-2">
                          <Link
                            to={`/sessions/${session.id}`}
                            className="text-console-text hover:text-console-accent"
                          >
                            {session.title ?? (
                              <span className="text-console-muted">untitled</span>
                            )}
                          </Link>
                          <LaunchSourceTag source={session.launch_source} />
                        </span>
                        {error !== null && (
                          <p className="text-state-failed text-xs">{error}</p>
                        )}
                      </td>
                      <td
                        className={`${CELL} text-console-muted hidden sm:table-cell`}
                      >
                        {session.kind}
                      </td>
                      <td
                        className={`${CELL} text-console-muted hidden md:table-cell`}
                      >
                        {profileNames.get(session.profile_id) ?? PLACEHOLDER}
                      </td>
                      <td
                        className={`${CELL} text-console-muted hidden font-mono text-xs lg:table-cell`}
                      >
                        {session.branch ?? PLACEHOLDER}
                      </td>
                      <td
                        className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap sm:table-cell`}
                      >
                        {formatRelative(session.last_activity_at)}
                      </td>
                      <td
                        className={`${CELL} text-console-muted text-right font-mono text-xs`}
                      >
                        {formatUsd(session.cost_usd, COST_DECIMALS)}
                      </td>
                      <td className={`${CELL} pr-0 text-right`}>
                        {finished && (
                          <button
                            type="button"
                            aria-label={`Delete ${sessionLabel(session)}`}
                            disabled={
                              remove.isPending || confirming === session.id
                            }
                            onClick={() => {
                              setRowError(null);
                              setConfirming(session.id);
                            }}
                            className="text-console-muted hover:text-state-failed font-mono text-xs disabled:opacity-50"
                          >
                            Delete
                          </button>
                        )}
                      </td>
                    </tr>

                    {confirming === session.id && (
                      <tr className={ROW}>
                        <td colSpan={COLUMNS.length} className={SPAN_CELL}>
                          <ConfirmPanel
                            message={`Delete ${sessionLabel(session)}? Its transcript and events go with it; the branch it left in the git mirror stays.`}
                            confirmLabel={`Delete ${sessionLabel(session)}`}
                            pending={remove.isPending}
                            onConfirm={() => {
                              remove.mutate(session.id);
                            }}
                            onCancel={() => {
                              setConfirming(null);
                            }}
                          />
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        )}
      </section>

      {/* Ahead/behind, merge, rebase and push for the session branches; the
          sessions above are what it acts on. */}
      <GitActionsPanel project={project} />
    </div>
  );
}
