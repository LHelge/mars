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
import { EmptyState } from "../components/EmptyState";
import type { HelpTopic } from "../help/topics";
import { LoadingState } from "../components/LoadingState";
import { PageLayout } from "../components/PageLayout";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { SectionHeader } from "../components/SectionHeader";
import { StatusBadge } from "../components/StatusBadge";
import { TableHead } from "../components/TableHead";
import {
  CELL,
  ROW_HOVER as ROW,
  SCROLLER,
  TABLE,
  type TableColumn,
} from "../components/tableStyles";
import { useAuth } from "../hooks/useAuth";
import { listProjects } from "../services/projects";
import { errorMessage } from "../services/errorMessage";
import { queryKeys } from "../services/queryKeys";
import { listSessions } from "../services/sessions";
import { listHumanTasks } from "../services/tasks";
import { listUsers } from "../services/users";
import type { Project, Session, Task } from "../types";
import { formatRelative, formatUsd, PLACEHOLDER } from "../utils/format";
import { taskPath } from "../tasks/taskLink";

/** `SPEC.md`, "Frontend", Dashboard: a 30-second refetch interval. */
export const DASHBOARD_REFETCH_MS = 30_000;

const SESSION_COLUMNS: readonly TableColumn[] = [
  { label: "State" },
  { label: "Session" },
  { label: "Project" },
  { label: "Kind", className: "hidden sm:table-cell" },
  { label: "Branch", className: "hidden md:table-cell" },
  { label: "Activity", className: "hidden sm:table-cell" },
  { label: "Cost", className: "pr-0 text-right" },
];

const TASK_COLUMNS: readonly TableColumn[] = [
  { label: "Task" },
  { label: "Title" },
  { label: "Project" },
  { label: "Priority" },
  { label: "Waiting for", className: "hidden lg:table-cell" },
  { label: "Assignee", className: "hidden sm:table-cell" },
  { label: "Attempts", className: "hidden sm:table-cell text-right" },
  { label: "Updated", className: "pr-0 text-right" },
];
// One link per row, stretched over its own cell — `LINK_CELL` is what
// positions it. Stretching it over the whole row would mean positioning the
// `<tr>`, and a table row is not reliably a containing block: where it is
// ignored, every row's `inset-0` resolves against some far ancestor instead
// and the last row drawn swallows clicks across the page.
const LINK_CELL = `${CELL} relative min-w-0`;
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
  /** A line under the title, for a section whose rows ask something of you. */
  description?: string;
  help?: HelpTopic;
  children: ReactNode;
}

/**
 * The four things every list on this page can be: loading its first page,
 * broken, empty, or a table. An error never blanks the table — the rows from
 * the last good poll stay on screen under the alert — and a list that has
 * failed is never called empty: "no running sessions" is something only a
 * successful read may say (`SPEC.md`, "Frontend", Read failures).
 */
function Section({
  title,
  query,
  rowCount,
  emptyTitle,
  emptyDescription,
  description,
  help,
  children,
}: SectionProps) {
  return (
    <section className="space-y-3">
      <SectionHeader title={title} description={description} help={help} />

      {query.isError && (
        <QueryErrorAlert
          query={query}
          message={`Could not load ${title.toLowerCase()}. ${errorMessage(query.error)}`}
        />
      )}

      {query.isPending ? (
        <LoadingState />
      ) : rowCount === 0 ? (
        query.isSuccess && (
          <EmptyState title={emptyTitle} description={emptyDescription} />
        )
      ) : (
        <div className={SCROLLER}>
          <table className={TABLE}>{children}</table>
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
      <TableHead columns={SESSION_COLUMNS} sticky />
      <tbody>
        {sessions.map((session) => (
          <tr key={session.id} className={ROW}>
            <td className={CELL}>
              <StatusBadge state={session.state} />
            </td>
            <td className={LINK_CELL}>
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
              className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap sm:table-cell`}
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
      <TableHead columns={TASK_COLUMNS} sticky />
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
              <td className={LINK_CELL}>
                <Link
                  to={taskPath(task.project_id, task.number)}
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
    queryFn: () => listSessions("running"),
    ...polled,
  });

  const parked = useQuery({
    queryKey: queryKeys.sessions.list("parked"),
    queryFn: () => listSessions("parked"),
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
          description="Tasks waiting in a project's human state. Resolve each on its task, then move it back to a queue state."
          help="task-flow"
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
