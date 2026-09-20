// The task drawer of `/projects/:id/tasks/:number` (`SPEC.md`, "Frontend",
// Routes and "Task board"): the board stays where it was and this panel opens
// over it, so closing the drawer is a navigation and never a reload.
//
// It reads the task itself, through TanStack Query keyed by
// `taskKeys.detail`. That key extends `taskKeys.all`, which `useTaskStream`
// invalidates on every task event (`SPEC.md`, "Board refresh ordering": "open
// task details and related queries are invalidated on task events as well"),
// so the drawer refetches on its own and handles no events.
//
// Layout: one column, ordered by how often it is read — what the task is, then
// what it says, then what it waits on, then what has been said about it. The
// metadata is a dense key/value grid in the console's monospace, the way the
// rest of the application writes ids and numbers; the prose is set in prose
// width so a long description stays readable in a narrow panel.
//
// Editing, state moves, release and "open in session" arrive in the actions
// task; the header keeps a slot for them beside `Copy link`.

import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import type { ReactNode } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";

import { Alert } from "../components/Alert";
import { CopyLinkButton } from "../components/CopyLinkButton";
import { LoadingState } from "../components/LoadingState";
import { MarkdownBody } from "../components/Markdown";
import { SubmitButton } from "../components/SubmitButton";
import { isNotFound, projectErrorMessage } from "../pages/project/messages";
import { getTask } from "../services/tasks";
import type { Task, TaskDetail as TaskDetailData } from "../types";
import { formatDateTime, formatRelative, shortId } from "../utils/format";
import { CommentForm } from "./CommentForm";
import { CommentList } from "./CommentList";
import { DependencyList } from "./DependencyList";
import { taskKeys } from "./queryKeys";
import { CHIP, PRIORITY_COLOUR, PRIORITY_MEANING } from "./taskChrome";
import { taskPath } from "./taskLink";
import { selectTaskById, useTaskStore } from "./taskStore";
import { useUsername } from "./useUsername";

export interface TaskDetailProps {
  projectId: string;
  /**
   * The `:number` of the route, or `null` when it is not a task number at all.
   * A route parameter that names no task is answered without a request.
   */
  number: number | null;
  /** The drawer's own actions, added by the tracker-actions task. */
  actions?: ReactNode;
}

export function TaskDetail({ projectId, number, actions }: TaskDetailProps) {
  const navigate = useNavigate();
  const [search] = useSearchParams();

  // Back to the board this drawer opened over, keeping whatever the board was
  // showing — the search field among it — rather than resetting the view.
  const close = useCallback(() => {
    const params = new URLSearchParams(search);
    params.set("tab", "board");
    void navigate(`/projects/${projectId}?${params.toString()}`);
  }, [navigate, projectId, search]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        close();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
    };
  }, [close]);

  const detail = useQuery({
    // `number ?? 0` is never requested: the query is disabled without one.
    queryKey: taskKeys.detail(projectId, number ?? 0),
    queryFn: () => getTask(projectId, number ?? 0),
    enabled: number !== null,
    retry: false,
  });

  const task = detail.data;

  return (
    <div className="fixed inset-0 z-40 flex justify-end">
      <div
        aria-hidden="true"
        onClick={close}
        className="bg-console-bg/70 absolute inset-0"
      />

      <aside
        role="dialog"
        aria-modal="true"
        aria-label={
          task === undefined ? "Task" : `Task #${String(task.number)}`
        }
        className="border-console-border bg-console-surface relative flex h-full w-full max-w-2xl flex-col overflow-y-auto border-l"
      >
        <header className="border-console-border bg-console-surface sticky top-0 z-10 flex flex-wrap items-baseline gap-x-3 gap-y-2 border-b px-4 py-3">
          <span className="text-console-muted font-mono text-sm">
            #{number === null ? "?" : number}
          </span>
          <h2 className="text-console-text min-w-0 flex-1 text-base">
            {task?.title ?? "Task"}
          </h2>
          <div className="flex shrink-0 items-center gap-2">
            {actions}
            {task !== undefined && (
              <CopyLinkButton
                path={taskPath(task.project_id, task.number)}
                label="Task link"
              />
            )}
            <SubmitButton
              type="button"
              variant="ghost"
              loading={false}
              onClick={close}
            >
              Close
            </SubmitButton>
          </div>
        </header>

        <div className="flex-1 px-4 py-4">
          {number === null || (detail.isError && isNotFound(detail.error)) ? (
            <NotFound onClose={close} />
          ) : detail.isPending ? (
            <LoadingState label="Loading the task" />
          ) : detail.isError ? (
            <Alert kind="error">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span>{projectErrorMessage(detail.error)}</span>
                <SubmitButton
                  type="button"
                  variant="ghost"
                  loading={detail.isFetching}
                  onClick={() => {
                    void detail.refetch();
                  }}
                >
                  Try again
                </SubmitButton>
              </div>
            </Alert>
          ) : (
            <TaskBody projectId={projectId} task={detail.data} />
          )}
        </div>
      </aside>
    </div>
  );
}

/**
 * The task named by the URL does not exist, or stopped existing while the
 * drawer was open: a `deleted` event invalidates the detail and the refetch
 * answers 404 (`SPEC.md`, "Board refresh ordering").
 */
function NotFound({ onClose }: { onClose: () => void }) {
  return (
    <div className="space-y-3">
      <p className="text-console-text text-sm">Task not found</p>
      <p className="text-console-muted max-w-prose text-sm">
        It was deleted, or this link names a task that never existed in this
        project.
      </p>
      <SubmitButton type="button" variant="ghost" loading={false} onClick={onClose}>
        Back to board
      </SubmitButton>
    </div>
  );
}

function TaskBody({
  projectId,
  task,
}: {
  projectId: string;
  task: TaskDetailData;
}) {
  return (
    <div className="space-y-6">
      <Meta projectId={projectId} task={task} />

      <Section title="Description">
        {task.description === null || task.description.trim() === "" ? (
          <p className="text-console-muted text-sm">No description</p>
        ) : (
          <div className="text-console-text max-w-prose text-sm">
            <MarkdownBody>{task.description}</MarkdownBody>
          </div>
        )}
      </Section>

      <Section title="Dependencies">
        <DependencyList
          projectId={projectId}
          dependsOn={task.depends_on}
          blocks={task.blocks}
          known={task.children}
        />
      </Section>

      <Section title="Children">
        {task.children.length === 0 ? (
          <p className="text-console-muted text-sm">No child tasks.</p>
        ) : (
          <ul className="space-y-1">
            {task.children.map((child) => (
              <ChildRow key={child.id} projectId={projectId} child={child} />
            ))}
          </ul>
        )}
      </Section>

      <Section title="Sessions">
        <Sessions task={task} />
      </Section>

      <Section title="Comments">
        <div className="space-y-4">
          <CommentList comments={task.comments} />
          <CommentForm projectId={projectId} number={task.number} />
        </div>
      </Section>
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-2">
      <h3 className="border-console-border text-console-text border-b pb-1 text-sm font-semibold tracking-tight">
        {title}
      </h3>
      {children}
    </section>
  );
}

/**
 * What the task is, as a key/value grid. Everything here is either a fact the
 * board also shows or a timestamp; nothing here is an action.
 */
function Meta({ projectId, task }: { projectId: string; task: TaskDetailData }) {
  const parent = useTaskStore(
    task.parent_id === null ? selectNothing : selectTaskById(task.parent_id),
  );
  const assignee = useUsername(task.assignee_user_id);

  return (
    <div className="space-y-2">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <Row label="State">
          <span className="text-console-text font-mono text-xs">
            {task.state}
          </span>
          {task.blocked && (
            <span
              className={`${CHIP} border-state-failed/60 text-state-failed ml-2`}
              title="Waiting on open children or unsatisfied dependencies"
            >
              blocked
            </span>
          )}
        </Row>

        <Row label="Priority">
          <span
            title={PRIORITY_MEANING[task.priority]}
            className={`font-mono text-xs ${PRIORITY_COLOUR[task.priority]}`}
          >
            P{task.priority}
          </span>
        </Row>

        {task.lease_holder_session_id !== null && (
          <Row label="Held by">
            <Link
              to={`/sessions/${task.lease_holder_session_id}`}
              title={task.lease_holder_session_id}
              className="text-console-accent font-mono text-xs hover:underline"
            >
              {shortId(task.lease_holder_session_id, 8)}
            </Link>
            {task.lease_since !== null && (
              <span className="text-console-muted ml-2 text-xs">
                since{" "}
                <time
                  dateTime={task.lease_since}
                  title={formatDateTime(task.lease_since)}
                >
                  {formatRelative(task.lease_since)}
                </time>
              </span>
            )}
          </Row>
        )}

        {task.attempts > 1 && (
          <Row label="Attempts">
            <span
              className="text-console-text font-mono text-xs"
              title="Sessions that have picked this task up"
            >
              {task.attempts}
            </span>
          </Row>
        )}

        {assignee !== null && (
          <Row label="Assignee">
            <span className="text-console-text font-mono text-xs">
              @{assignee}
            </span>
          </Row>
        )}

        {task.parent_id !== null && (
          <Row label="Parent">
            {parent === undefined ? (
              <span className="text-console-muted font-mono text-xs">
                part of #?
              </span>
            ) : (
              <Link
                to={taskPath(projectId, parent.number)}
                className="text-console-accent font-mono text-xs hover:underline"
              >
                part of #{parent.number}
              </Link>
            )}
          </Row>
        )}

        {task.labels.length > 0 && (
          <Row label="Labels">
            <span className="flex flex-wrap gap-1">
              {task.labels.map((label) => (
                <span key={label} className={`${CHIP} text-console-muted`}>
                  {label}
                </span>
              ))}
            </span>
          </Row>
        )}

        <Row label="Created">
          <Stamp iso={task.created_at} />
        </Row>
        <Row label="Updated">
          <Stamp iso={task.updated_at} />
        </Row>
        {task.closed_at !== null && (
          <Row label="Closed">
            <Stamp iso={task.closed_at} />
          </Row>
        )}
      </dl>

      {task.needs_human_reason !== null && (
        <p className="text-state-human border-state-human/50 max-w-prose border-l-2 pl-2 text-sm">
          {task.needs_human_reason}
        </p>
      )}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-console-muted text-xs">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  );
}

function Stamp({ iso }: { iso: string }) {
  return (
    <time
      dateTime={iso}
      title={formatDateTime(iso)}
      className="text-console-text text-xs"
    >
      {formatRelative(iso)}
    </time>
  );
}

function ChildRow({ projectId, child }: { projectId: string; child: Task }) {
  return (
    <li>
      <Link
        to={taskPath(projectId, child.number)}
        className="border-console-border bg-console-bg hover:border-console-accent/60 flex items-baseline gap-2 rounded border px-2 py-1.5"
      >
        <span className="text-console-muted shrink-0 font-mono text-xs">
          #{child.number}
        </span>
        <span className="text-console-text min-w-0 flex-1 truncate text-sm">
          {child.title}
        </span>
        <span className="text-console-muted shrink-0 font-mono text-[0.6875rem]">
          {child.state}
        </span>
        {child.blocked && (
          <span className={`${CHIP} border-state-failed/60 text-state-failed`}>
            blocked
          </span>
        )}
      </Link>
    </li>
  );
}

/**
 * Every session that has touched the task, with the one holding it now marked
 * (`SPEC.md`, "Frontend", "Task board": links into their transcripts).
 */
function Sessions({ task }: { task: TaskDetailData }) {
  if (task.sessions.length === 0) {
    return (
      <p className="text-console-muted text-sm">No session has touched this task.</p>
    );
  }

  return (
    <ul className="space-y-1">
      {task.sessions.map((touch) => {
        const holding = touch.session_id === task.lease_holder_session_id;
        return (
          <li
            key={touch.session_id}
            className={`flex flex-wrap items-baseline gap-x-3 gap-y-1 rounded px-2 py-1 ${holding ? "border-console-accent/60 bg-console-accent/5 border" : ""}`}
          >
            <Link
              to={`/sessions/${touch.session_id}`}
              title={touch.session_id}
              className="text-console-accent font-mono text-xs hover:underline"
            >
              {shortId(touch.session_id, 8)}
            </Link>
            {holding && (
              <span
                className={`${CHIP} border-console-accent/60 text-console-accent`}
              >
                holding
              </span>
            )}
            <span className="text-console-muted text-xs">
              first{" "}
              <time
                dateTime={touch.first_touched_at}
                title={formatDateTime(touch.first_touched_at)}
              >
                {formatRelative(touch.first_touched_at)}
              </time>
            </span>
            <span className="text-console-muted text-xs">
              last{" "}
              <time
                dateTime={touch.last_touched_at}
                title={formatDateTime(touch.last_touched_at)}
              >
                {formatRelative(touch.last_touched_at)}
              </time>
            </span>
          </li>
        );
      })}
    </ul>
  );
}

/** No parent, no lookup: a selector that subscribes to nothing that changes. */
function selectNothing(): undefined {
  return undefined;
}
