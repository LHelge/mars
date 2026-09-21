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
// quiet — and stops entirely while the browser tab is in the background.
//
// Deleting is only offered for a session that has finished; the API refuses
// any other (`DELETE /sessions/{id}`: must be `done` or `failed`), and the
// refusal is shown on the row it belongs to rather than at the top of the page.

import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Link } from "react-router";
import {
  EmptyState,
  GitActionsPanel,
  LoadingState,
  QueryErrorAlert,
  SessionStatePill,
} from "../../components";
import { ApiError } from "../../services/apiClient";
import { listProfiles } from "../../services/profiles";
import { queryKeys } from "../../services/queryKeys";
import { deleteSession, listProjectSessions } from "../../services/sessions";
import type { Session, SessionState } from "../../types";
import { formatRelative, formatUsd, PLACEHOLDER } from "../../utils/format";
import { LaunchSessionForm } from "./LaunchSessionForm";
import type { ProjectTabPanelProps } from "./tabs";

/** While a session is still starting or working, its row changes on its own. */
const BUSY_POLL_MS = 10_000;
/** Once everything has settled, the list only has to stay roughly current. */
const IDLE_POLL_MS = 60_000;

/** A session's spend is often a fraction of a cent. */
const COST_DECIMALS = 4;

type Filter = SessionState | "all";

const FILTERS: { value: Filter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "parked", label: "Parked" },
  { value: "done", label: "Done" },
  { value: "failed", label: "Failed" },
];

const CELL = "py-1.5 pr-3 align-middle";
const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";
const ROW = "border-console-border/60 border-b last:border-b-0";

function isBusy(session: Session): boolean {
  return session.state === "creating" || session.state === "running";
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

  const base = queryKeys.projects.sessions(project.id);

  const sessions = useQuery({
    queryKey: filter === "all" ? base : [...base, filter],
    queryFn: () =>
      listProjectSessions(project.id, filter === "all" ? undefined : filter),
    placeholderData: keepPreviousData,
    refetchIntervalInBackground: false,
    refetchInterval: (query) =>
      (query.state.data ?? []).some(isBusy) ? BUSY_POLL_MS : IDLE_POLL_MS,
  });

  // Only for the profile column; the launch form reads the same cached list.
  const profiles = useQuery({
    queryKey: queryKeys.projects.profiles(project.id),
    queryFn: () => listProfiles(project.id),
  });

  const profileNames = useMemo(
    () =>
      new Map<string, string>(
        (profiles.data ?? []).map((profile) => [profile.id, profile.name]),
      ),
    [profiles.data],
  );

  const rows = useMemo(
    () =>
      [...(sessions.data ?? [])].sort((a, b) =>
        b.created_at.localeCompare(a.created_at),
      ),
    [sessions.data],
  );

  const remove = useMutation({
    mutationFn: (id: string) => deleteSession(id),
    onSuccess: () => {
      setConfirming(null);
      setRowError(null);
      void queryClient.invalidateQueries({ queryKey: base });
    },
    onError: (caught: unknown, id: string) => {
      setConfirming(null);
      setRowError({
        id,
        message:
          caught instanceof ApiError
            ? caught.error
            : "Could not delete the session",
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
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-console-border border-b">
                <th scope="col" className={HEAD}>
                  State
                </th>
                <th scope="col" className={HEAD}>
                  Session
                </th>
                <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
                  Kind
                </th>
                <th scope="col" className={`${HEAD} hidden md:table-cell`}>
                  Profile
                </th>
                <th scope="col" className={`${HEAD} hidden lg:table-cell`}>
                  Branch
                </th>
                <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
                  Activity
                </th>
                <th scope="col" className={`${HEAD} text-right`}>
                  Cost
                </th>
                <th scope="col" className={`${HEAD} pr-0 text-right`}>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((session) => {
                const finished =
                  session.state === "done" || session.state === "failed";
                const error =
                  rowError?.id === session.id ? rowError.message : null;

                return (
                  <tr key={session.id} className={ROW}>
                    <td className={CELL}>
                      <SessionStatePill
                        state={session.state}
                        error={session.error}
                      />
                    </td>
                    <td className={`${CELL} min-w-0`}>
                      <Link
                        to={`/sessions/${session.id}`}
                        className="text-console-text hover:text-console-accent"
                      >
                        {session.title ?? (
                          <span className="text-console-muted">untitled</span>
                        )}
                      </Link>
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
                          disabled={remove.isPending}
                          onClick={() => {
                            setRowError(null);
                            if (confirming === session.id) {
                              remove.mutate(session.id);
                            } else {
                              setConfirming(session.id);
                            }
                          }}
                          className="text-console-muted hover:text-state-failed font-mono text-xs disabled:opacity-50"
                        >
                          {confirming === session.id
                            ? "Confirm delete"
                            : "Delete"}
                        </button>
                      )}
                    </td>
                  </tr>
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
