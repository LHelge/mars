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
// Editing, state moves, release and delete are `TaskActions`, the bar at the
// top of the body. It lives there rather than in the header because every one
// of those actions needs the loaded task, and because the panels they open —
// a confirmation, the edit form — need the width of the body.
//
// It is a native modal `<dialog>`, opened with `showModal()`, which is what
// makes it a modal rather than a panel that merely says it is one: the browser
// moves focus into it, keeps Tab and Shift+Tab inside it, makes the board
// behind it inert to pointer and keyboard alike, and gives focus back to the
// card that opened it when it closes. What the browser does not decide is
// where focus lands *inside* the drawer — the heading, so the task is
// announced and Tab walks the panel from the top, including after a link to
// another task has remounted the body under it — and when Escape may close
// anything: `drawerEscape.ts`.

import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";

import { CopyLinkButton } from "../components/CopyLinkButton";
import { LoadingState } from "../components/LoadingState";
import { MarkdownBody } from "../components/Markdown";
import { QueryErrorAlert } from "../components/QueryErrorAlert";
import { SubmitButton } from "../components/SubmitButton";
import { errorMessage, isNotFound } from "../services/errorMessage";
import { getTask } from "../services/tasks";
import type { Task, TaskDetail as TaskDetailData } from "../types";
import { formatDateTime, formatRelative, shortId } from "../utils/format";
import { taskCardTestId } from "../utils/testIds";
import { CommentForm } from "./CommentForm";
import { CommentList } from "./CommentList";
import { DependencyEditor } from "./DependencyEditor";
import {
  createEscapeRegistry,
  DrawerEscapeContext,
  escapeAction,
  hasDraftText,
} from "./drawerEscape";
import { HandoffPanel } from "./HandoffPanel";
import { taskKeys } from "./queryKeys";
import { TaskActions } from "./TaskActions";
import { TaskEditForm } from "./TaskEditForm";
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
}

export function TaskDetail({ projectId, number }: TaskDetailProps) {
  const navigate = useNavigate();
  const [search] = useSearchParams();

  const dialogRef = useRef<HTMLDialogElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  /** Has anyone typed inside the drawer? A pre-filled field is not a draft. */
  const typed = useRef(false);
  /** The task the drawer is showing, readable from an unmount cleanup. */
  const shown = useRef(number);
  /** An Escape over unsaved text has asked; a second one discards it. */
  const [armed, setArmed] = useState(false);

  useEffect(() => {
    shown.current = number;
  }, [number]);

  // Back to the board this drawer opened over, keeping whatever the board was
  // showing — the search field among it — rather than resetting the view.
  const close = useCallback(() => {
    const params = new URLSearchParams(search);
    params.set("tab", "board");
    void navigate(`/projects/${projectId}?${params.toString()}`);
  }, [navigate, projectId, search]);

  // Modal for as long as the drawer is mounted. Closing it is a navigation,
  // so the close happens here, in the unmount, where the element is still in
  // the document and the browser can hand focus back.
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog === null) return;
    if (!dialog.open) dialog.showModal();
    return () => {
      if (!dialog.open) return;
      dialog.close();
      restoreFocus(dialog, shown.current);
    };
  }, []);

  // Focus goes to the heading when the drawer opens and again whenever it
  // shows another task: a click on a child or parent link replaces the body
  // under the cursor, and the link that was focused goes with it.
  useEffect(() => {
    headingRef.current?.focus();
  }, [number]);

  // One registry for the life of the drawer: every sub-form under it — the
  // edit form, the hand-off forms, the merge form, the launch panel and the
  // two confirmations — registers its own `close` here while it is open
  // (`drawerEscape.ts`).
  const escapeRegistry = useMemo(() => createEscapeRegistry(), []);

  // Escape is read from the document rather than from the dialog element: the
  // drawer is modal, so every key press belongs to it, including the ones that
  // arrive with nothing focused because the control that had focus — a
  // confirmation's button, a form that just saved — removed itself.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        // Any other key is the user carrying on: the question lapses.
        if (armed) setArmed(false);
        return;
      }

      const dialog = dialogRef.current;
      const action = escapeAction({
        defaultPrevented: event.defaultPrevented,
        composing: event.isComposing,
        dirty: typed.current && dialog !== null && hasDraftText(dialog),
        armed,
      });
      if (action === "ignore") return;

      // Ours from here on, so the browser's own close request never fires.
      event.preventDefault();
      if (action === "confirm") {
        setArmed(true);
        return;
      }

      setArmed(false);
      const innermost = escapeRegistry.innermost();
      if (innermost === undefined) {
        close();
        return;
      }
      // The form is about to take its fields — and whatever is focused among
      // them — out of the document, so focus goes back to the heading first.
      headingRef.current?.focus();
      innermost();
    };

    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
    };
  }, [armed, close, escapeRegistry]);

  const detail = useQuery({
    // `number ?? 0` is never requested: the query is disabled without one.
    queryKey: taskKeys.detail(projectId, number ?? 0),
    queryFn: () => getTask(projectId, number ?? 0),
    enabled: number !== null,
  });

  const task = detail.data;

  return (
    // The dialog fills the viewport, with the panel at its right edge and the
    // dimming overlay — which is what a click outside the panel lands on —
    // filling the rest. The overlay is an element rather than `::backdrop`
    // because a pseudo-element does not inherit the theme's custom properties
    // everywhere, and this one has to close the drawer anyway.
    <dialog
      ref={dialogRef}
      aria-label={task === undefined ? "Task" : `Task #${String(task.number)}`}
      onCancel={(event) => {
        // Escape is decided by the keydown handler above; the browser's own
        // close request never gets to skip the unsaved-text question.
        event.preventDefault();
      }}
      onInput={() => {
        typed.current = true;
        if (armed) setArmed(false);
      }}
      className="fixed inset-0 z-40 m-0 flex h-full max-h-none w-full max-w-none justify-end border-0 bg-transparent p-0 text-inherit"
    >
      <div
        aria-hidden="true"
        onClick={close}
        className="bg-console-bg/70 absolute inset-0"
      />

      <aside className="border-console-border bg-console-surface relative flex h-full w-full max-w-2xl flex-col overflow-y-auto border-l">
        <header className="border-console-border bg-console-surface sticky top-0 z-10 flex flex-wrap items-baseline gap-x-3 gap-y-2 border-b px-4 py-3">
          <span className="text-console-muted font-mono text-sm">
            #{number === null ? "?" : number}
          </span>
          <h2
            ref={headingRef}
            tabIndex={-1}
            className="text-console-text min-w-0 flex-1 text-base outline-none"
          >
            {task?.title ?? "Task"}
          </h2>
          <div className="flex shrink-0 items-center gap-2">
            {task !== undefined && (
              <CopyLinkButton
                path={taskPath(task.project_id, task.number)}
                label="Task link"
              />
            )}
            <SubmitButton
              type="button"
              variant="ghost"
              onClick={close}
            >
              Close
            </SubmitButton>
          </div>
          {armed && (
            <p role="status" className="text-state-human w-full text-xs">
              Unsaved text here. Press Escape again to discard it.
            </p>
          )}
        </header>

        <div className="flex-1 px-4 py-4">
          {number === null || (detail.isError && isNotFound(detail.error)) ? (
            <NotFound onClose={close} />
          ) : task === undefined ? (
            detail.isPending ? (
              <LoadingState label="Loading the task" />
            ) : (
              <QueryErrorAlert
                query={detail}
                message={errorMessage(detail.error)}
              />
            )
          ) : (
            // A failed refetch keeps the drawer and whatever is being edited
            // in it (`SPEC.md`, "Frontend", Read failures).
            <div className="space-y-3">
              {detail.isError && (
                <QueryErrorAlert
                  query={detail}
                  message={errorMessage(detail.error)}
                />
              )}
              {/* The task's id is the drawer's identity boundary: everything
                  under `TaskBody` — edit mode and its draft, the comment box,
                  an open confirmation, a half-written review — belongs to the
                  task it was written for, and a cached neighbour renders with
                  no loading state in between to unmount it. Keying by
                  `task.id` makes "another task" a remount and "the same task,
                  refetched" — which every task event causes — leave the draft
                  alone (`SPEC.md`, "Frontend", "Task board"). */}
              <DrawerEscapeContext.Provider value={escapeRegistry}>
                <TaskBody key={task.id} projectId={projectId} task={task} />
              </DrawerEscapeContext.Provider>
            </div>
          )}
        </div>
      </aside>
    </dialog>
  );
}

/**
 * Where focus goes once the drawer is gone.
 *
 * Closing the dialog is the browser's cue to give focus back to whatever had
 * it when `showModal()` ran, which for a card click is that card's own link —
 * the one place a user expects to carry on from. Two cases have no such
 * element: a drawer opened from a pasted link, where focus was on nothing, and
 * one whose opener has since been deleted or replaced. Then the board's own
 * card for the task is the next best thing, and the page's main region the one
 * after that; landing on `<body>` would drop a keyboard user at the top of the
 * document.
 */
function restoreFocus(dialog: HTMLDialogElement, number: number | null): void {
  const active = document.activeElement;
  if (
    active instanceof HTMLElement &&
    active !== document.body &&
    !dialog.contains(active)
  ) {
    return;
  }

  const card =
    number === null
      ? null
      : document.querySelector<HTMLElement>(
          `[data-testid="${taskCardTestId(number)}"] a`,
        );
  (card ?? document.querySelector<HTMLElement>("main"))?.focus();
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
      <SubmitButton type="button" variant="ghost" onClick={onClose}>
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
  // Edit mode replaces the fields it edits rather than sitting beside them, so
  // the drawer never shows a title twice with two different values in it.
  const [editing, setEditing] = useState(false);

  // Escape shuts an open sub-form before it shuts the drawer; over a draft it
  // asks first, so nothing here is emptied by one key press. Each form claims
  // it in the module that owns its open state (`drawerEscape.ts`), which for a
  // form that is mounted only while it is open is the form itself.

  return (
    <div className="space-y-6">
      <TaskActions
        projectId={projectId}
        task={task}
        editing={editing}
        onEditingChange={setEditing}
      />

      {editing ? (
        <TaskEditForm
          projectId={projectId}
          task={task}
          onDone={() => {
            setEditing(false);
          }}
        />
      ) : (
        <>
          <Meta projectId={projectId} task={task} />

          <Section title="Description">
            {task.description === null || task.description.trim() === "" ? (
              <p className="text-console-muted text-sm">No description</p>
            ) : (
              <div className="text-console-text text-sm">
                <MarkdownBody>{task.description}</MarkdownBody>
              </div>
            )}
          </Section>
        </>
      )}

      <Section title="Dependencies">
        <DependencyEditor projectId={projectId} task={task} />
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

      <Section title="Code hand-off">
        <HandoffPanel projectId={projectId} task={task} />
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
