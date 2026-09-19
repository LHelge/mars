// `SPEC.md`, "Frontend", Dashboard: the home page loads
// `GET /sessions?state=running`, `GET /sessions?state=parked` and
// `GET /tasks?state_kind=human` through TanStack Query on a 30-second refetch,
// and links each row to its session or task.
//
// The three lists are the operator's working set, so they are tables, not
// cards: one line per session or task, identifiers in mono, and the only
// colour on the page carried by the state badge and a P0 priority. Polling
// stops while the tab is hidden and a window focus refreshes instead
// (`refetchIntervalInBackground: false`).

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import type { UseQueryResult } from "@tanstack/react-query";
import { useMemo } from "react";
import type { ReactNode } from "react";
import { Link } from "react-router";
import {
  Alert,
  EmptyState,
  LoadingState,
  PageLayout,
  SectionHeader,
  StatusBadge,
  SubmitButton,
} from "../components";
import { useAuth } from "../hooks/useAuth";
import { listProjects } from "../services/projects";
import { queryKeys } from "../services/queryKeys";
import { listSessions } from "../services/sessions";
import { listHumanTasks } from "../services/tasks";
import { listUsers } from "../services/users";
import type { Project, Session, Task } from "../types";
import { formatRelative, formatUsd, PLACEHOLDER } from "../utils/format";

/** `SPEC.md`, "Frontend", Dashboard: a 30-second refetch interval. */
export const DASHBOARD_REFETCH_MS = 30_000;

const CELL = "py-1.5 pr-3 align-middle";
const HEAD = "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";
const ROW =
  "border-console-border/60 hover:bg-console-raised/60 relative border-b last:border-b-0";
// One link per row, stretched over the whole row: the row is a single tab stop
// and a single click target, without nesting anything inside an anchor.
const ROW_LINK =
  "text-console-text hover:text-console-accent after:absolute after:inset-0 after:content-['']";

function projectLabel(id: string, projects: Map<string, Project>): string {
  return projects.get(id)?.name ?? id.slice(0, 8);
}

/** Newest first; a row with no timestamp sorts to the bottom. */
function byTimestampDesc<T>(rows: T[], at: (row: T) => string | null): T[] {
  return [...rows].sort((a, b) => (at(b) ?? "").localeCompare(at(a) ?? ""));
}

function byPriorityThenUpdated(a: Task, b: Task): number {
  return a.priority - b.priority || b.updated_at.localeCompare(a.updated_at);
}

interface SectionProps {
  title: string;
  /** The query behind the section; only its status is read here. */
  query: UseQueryResult<unknown>;
  rowCount: number;
  emptyTitle: string;
  emptyDescription?: string;
  children: ReactNode;
}

/**
 * The four things every list on this page can be: loading its first page,
 * broken, empty, or a table. An error never blanks the table — the rows from
 * the last good poll stay on screen under the alert.
 */
function Section({
  title,
  query,
  rowCount,
  emptyTitle,
  emptyDescription,
  children,
}: SectionProps) {
  return (
    <section className="space-y-3">
      <SectionHeader title={title} />

      {query.isError && (
        <Alert kind="error">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>{`Could not load ${title.toLowerCase()}. ${query.error instanceof Error ? query.error.message : ""}`.trim()}</span>
            <SubmitButton
              type="button"
              variant="ghost"
              loading={query.isFetching}
              onClick={() => {
                void query.refetch();
              }}
            >
              Try again
            </SubmitButton>
          </div>
        </Alert>
      )}

      {query.isPending ? (
        <LoadingState />
      ) : rowCount === 0 ? (
        <EmptyState title={emptyTitle} description={emptyDescription} />
      ) : (
        <div className="max-h-96 overflow-y-auto">
          <table className="w-full border-collapse text-sm">{children}</table>
        </div>
      )}
    </section>
  );
}

function SessionRows({
  sessions,
  projects,
}: {
  sessions: Session[];
  projects: Map<string, Project>;
}) {
  return (
    <>
      <thead className="bg-console-bg sticky top-0 z-10">
        <tr className="border-console-border border-b">
          <th scope="col" className={HEAD}>
            State
          </th>
          <th scope="col" className={HEAD}>
            Session
          </th>
          <th scope="col" className={HEAD}>
            Project
          </th>
          <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
            Kind
          </th>
          <th scope="col" className={`${HEAD} hidden md:table-cell`}>
            Branch
          </th>
          <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
            Activity
          </th>
          <th scope="col" className={`${HEAD} pr-0 text-right`}>
            Cost
          </th>
        </tr>
      </thead>
      <tbody>
        {sessions.map((session) => (
          <tr key={session.id} className={ROW}>
            <td className={CELL}>
              <StatusBadge state={session.state} />
            </td>
            <td className={`${CELL} min-w-0`}>
              <Link to={`/sessions/${session.id}`} className={ROW_LINK}>
                {session.title ?? (
                  <span className="text-console-muted">Untitled session</span>
                )}
              </Link>
            </td>
            <td className={`${CELL} text-console-muted`}>
              {projectLabel(session.project_id, projects)}
            </td>
            <td className={`${CELL} text-console-muted hidden sm:table-cell`}>
              {session.kind}
            </td>
            <td
              className={`${CELL} text-console-muted hidden font-mono text-xs md:table-cell`}
            >
              {session.branch ?? PLACEHOLDER}
            </td>
            <td
              className={`${CELL} text-console-muted hidden font-mono text-xs sm:table-cell whitespace-nowrap`}
            >
              {formatRelative(session.last_activity_at)}
            </td>
            <td
              className={`${CELL} text-console-muted pr-0 text-right font-mono text-xs`}
            >
              {formatUsd(session.cost_usd)}
            </td>
          </tr>
        ))}
      </tbody>
    </>
  );
}

function TaskRows({
  tasks,
  projects,
  usernames,
}: {
  tasks: Task[];
  projects: Map<string, Project>;
  /** Empty unless the viewer is an admin and `GET /users` resolved. */
  usernames: Map<string, string>;
}) {
  return (
    <>
      <thead className="bg-console-bg sticky top-0 z-10">
        <tr className="border-console-border border-b">
          <th scope="col" className={HEAD}>
            Task
          </th>
          <th scope="col" className={HEAD}>
            Title
          </th>
          <th scope="col" className={HEAD}>
            Project
          </th>
          <th scope="col" className={HEAD}>
            Priority
          </th>
          <th scope="col" className={`${HEAD} hidden lg:table-cell`}>
            Waiting for
          </th>
          <th scope="col" className={`${HEAD} hidden sm:table-cell`}>
            Assignee
          </th>
          <th scope="col" className={`${HEAD} hidden sm:table-cell text-right`}>
            Attempts
          </th>
          <th scope="col" className={`${HEAD} pr-0 text-right`}>
            Updated
          </th>
        </tr>
      </thead>
      <tbody>
        {tasks.map((task) => {
          const assignee =
            task.assignee_user_id === null
              ? null
              : (usernames.get(task.assignee_user_id) ?? null);

          return (
            <tr key={task.id} className={ROW}>
              <td className={`${CELL} text-console-muted font-mono text-xs`}>
                #{task.number}
              </td>
              <td className={`${CELL} min-w-0`}>
                <Link
                  to={`/projects/${task.project_id}/tasks/${task.number}`}
                  className={ROW_LINK}
                >
                  {task.title}
                </Link>
              </td>
              <td className={`${CELL} text-console-muted`}>
                {projectLabel(task.project_id, projects)}
              </td>
              <td
                className={`${CELL} font-mono text-xs ${task.priority === 0 ? "text-state-failed" : "text-console-muted"}`}
              >
                P{task.priority}
              </td>
              <td
                className={`${CELL} text-console-muted hidden max-w-[28ch] lg:table-cell`}
              >
                <span
                  className="block truncate"
                  title={task.needs_human_reason ?? undefined}
                >
                  {task.needs_human_reason ?? PLACEHOLDER}
                </span>
              </td>
              <td className={`${CELL} text-console-muted hidden sm:table-cell`}>
                {assignee ?? PLACEHOLDER}
              </td>
              <td
                className={`${CELL} text-console-muted hidden text-right font-mono text-xs sm:table-cell`}
              >
                {task.attempts}
              </td>
              <td
                className={`${CELL} text-console-muted pr-0 text-right font-mono text-xs whitespace-nowrap`}
              >
                {formatRelative(task.updated_at)}
              </td>
            </tr>
          );
        })}
      </tbody>
    </>
  );
}

export function DashboardPage() {
  const { isAdmin } = useAuth();

  const polled = {
    refetchInterval: DASHBOARD_REFETCH_MS,
    refetchIntervalInBackground: false,
    placeholderData: keepPreviousData,
  } as const;

  const running = useQuery({
    queryKey: queryKeys.sessions.list("running"),
    queryFn: () => listSessions({ state: "running" }),
    ...polled,
  });

  const parked = useQuery({
    queryKey: queryKeys.sessions.list("parked"),
    queryFn: () => listSessions({ state: "parked" }),
    ...polled,
  });

  const humanTasks = useQuery({
    queryKey: queryKeys.tasks.human(),
    queryFn: listHumanTasks,
    ...polled,
  });

  // Project names change about never, so this one rides the default interval.
  const projects = useQuery({
    queryKey: queryKeys.projects.list(),
    queryFn: listProjects,
  });

  // `GET /users` is admin only; for everyone else an assignee id stays hidden
  // rather than being shown raw.
  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    enabled: isAdmin,
  });

  const projectsById = useMemo(
    () => new Map<string, Project>((projects.data ?? []).map((p) => [p.id, p])),
    [projects.data],
  );

  const usernamesById = useMemo(
    () =>
      new Map<string, string>(
        (users.data ?? []).map((u) => [u.id, u.username]),
      ),
    [users.data],
  );

  const runningRows = useMemo(
    () => byTimestampDesc(running.data ?? [], (s) => s.last_activity_at),
    [running.data],
  );

  const parkedRows = useMemo(
    () => byTimestampDesc(parked.data ?? [], (s) => s.parked_at),
    [parked.data],
  );

  const taskRows = useMemo(
    () => [...(humanTasks.data ?? [])].sort(byPriorityThenUpdated),
    [humanTasks.data],
  );

  return (
    <PageLayout title="Dashboard">
      <div className="space-y-8">
        <Section
          title="Running sessions"
          query={running}
          rowCount={runningRows.length}
          emptyTitle="No running sessions"
          emptyDescription="Start one from a project to see it here."
        >
          <SessionRows sessions={runningRows} projects={projectsById} />
        </Section>

        <Section
          title="Parked sessions"
          query={parked}
          rowCount={parkedRows.length}
          emptyTitle="No parked sessions"
          emptyDescription="A conversational session parks when its agent finishes a turn."
        >
          <SessionRows sessions={parkedRows} projects={projectsById} />
        </Section>

        <Section
          title="Needs a human"
          query={humanTasks}
          rowCount={taskRows.length}
          emptyTitle="Nothing is waiting for a human"
          emptyDescription="Tasks an agent escalates land here."
        >
          <TaskRows
            tasks={taskRows}
            projects={projectsById}
            usernames={usernamesById}
          />
        </Section>
      </div>
    </PageLayout>
  );
}
