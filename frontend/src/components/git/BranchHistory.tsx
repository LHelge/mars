// The first-parent history of an integration head on the Branches tab
// (`SPEC.md`, "Git": `HistoryEntry`; "Frontend", Project page): one row per
// commit, newest first, with who asked for it and the tasks and sessions
// behind it, and "Revert to here" on every row but the head's, one a later
// revert undid (muted, linking to that revert's row) and one that already
// holds the head's content (`history.ts`, `historyRowMark`).
//
// It reads like `git log --first-parent`: a merge of a task's hand-off is one
// row, attributed to that task, so "what went into main after this point" is
// the rows above a row. That is what a revert undoes, and what its
// confirmation lists (`RevertConfirm.tsx`).
//
// A successful revert leaves a note above the table naming the new commit and
// where it goes next: it waits on the head until someone pushes it, with the
// head's own `Push…`; the new top row, the revert itself, is scrolled to and
// highlighted, so the change is visible in the table too. The note is the answer to the last revert and is gone
// the moment another one is opened or the head changes.

import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Link } from "react-router";

import { taskPath } from "../../tasks/taskLink";
import { useUsername } from "../../tasks/useUsername";
import type { HistoryEntry, RevertResult } from "../../types";
import {
  formatDateTime,
  formatRelative,
  PLACEHOLDER,
  shortSha,
} from "../../utils/format";
import { Alert } from "../Alert";
import { EmptyState } from "../EmptyState";
import { FieldShell } from "../FieldShell";
import { CONTROL, TAP_INLINE } from "../fieldStyles";
import { Icon, ICON_CLASS } from "../icons";
import { LoadingState } from "../LoadingState";
import { QueryErrorAlert } from "../QueryErrorAlert";
import { SubmitButton } from "../SubmitButton";
import { TableHead } from "../TableHead";
import {
  CELL_TOP,
  ROW,
  SPAN_CELL_ROOMY,
  TABLE,
  X_SCROLLER,
  type TableColumn,
} from "../tableStyles";
import type { ReportBusy } from "./formState";
import {
  historyRowId,
  historyRowMark,
  parseRequestedBy,
  revertRange,
  type HistoryRowMark,
} from "./history";
import type { IntegrationHead } from "./integrationHeads";
import { RevertConfirm } from "./RevertConfirm";
import { useBranchHistory } from "./useBranchHistory";

const COLUMNS: readonly TableColumn[] = [
  { label: "Commit" },
  { label: "Subject" },
  { label: "Requested by", className: "hidden md:table-cell" },
  { label: "Tasks and sessions" },
  { label: "When", className: "hidden sm:table-cell" },
  { label: "Actions", className: "pr-0 text-right", srOnly: true },
];

export interface BranchHistoryProps {
  projectId: string;
  /** The integration heads, the default one first. */
  heads: IntegrationHead[];
  /** Another git form is in flight. */
  disabled: boolean;
  onBusy: ReportBusy;
}

interface LastRevert {
  branch: string;
  to: string;
  result: RevertResult;
}

export function BranchHistory({
  projectId,
  heads,
  disabled,
  onBusy,
}: BranchHistoryProps) {
  const [chosen, setChosen] = useState<string | null>(null);
  /** The row whose revert confirmation is open, by commit. */
  const [open, setOpen] = useState<string | null>(null);
  const [last, setLast] = useState<LastRevert | null>(null);
  /** The row drawn highlighted and scrolled to, by commit. */
  const [highlight, setHighlight] = useState<string | null>(null);

  const branch =
    chosen !== null && heads.some((head) => head.name === chosen)
      ? chosen
      : (heads[0]?.name ?? null);
  const history = useBranchHistory(projectId, branch);
  const entries = history.data?.pages.flat() ?? [];
  const head = entries[0];

  if (branch === null) {
    return null;
  }

  const formId = `git-revert-${branch}`;

  return (
    <div className="space-y-2">
      {heads.length > 1 && (
        <div className="max-w-xs">
          <FieldShell label="Head" name="git-history-head">
            {(control) => (
              <select
                {...control}
                value={branch}
                onChange={(event) => {
                  setChosen(event.target.value);
                  setOpen(null);
                  setLast(null);
                  setHighlight(null);
                }}
                className={`${CONTROL} font-mono`}
              >
                {heads.map((item) => (
                  <option key={item.name} value={item.name}>
                    {item.name}
                  </option>
                ))}
              </select>
            )}
          </FieldShell>
        </div>
      )}

      {last !== null && (
        <Alert
          kind="success"
          onDismiss={() => {
            setLast(null);
          }}
        >
          Reverted <span className="font-mono">{last.branch}</span> to{" "}
          <span className="font-mono">{shortSha(last.to)}</span> with commit{" "}
          <span className="font-mono">{shortSha(last.result.commit)}</span>
          {last.result.reopened.length > 0 &&
            `, reopening ${String(last.result.reopened.length)} ${
              last.result.reopened.length === 1 ? "task" : "tasks"
            }`}
          . It waits on <span className="font-mono">{last.branch}</span> until
          it is pushed: use <strong>Push…</strong> on its row under Integration
          heads.
        </Alert>
      )}

      {history.isRefetchError && entries.length > 0 && (
        <QueryErrorAlert
          message="Could not refresh the history."
          query={history}
        />
      )}

      {history.isPending ? (
        <LoadingState label="Loading history" />
      ) : history.isError && entries.length === 0 ? (
        <QueryErrorAlert
          message={`Could not load the history of ${branch}.`}
          query={history}
        />
      ) : entries.length === 0 ? (
        <EmptyState
          title="No commits yet"
          description={`${branch} has no history to show.`}
        />
      ) : (
        <div className={X_SCROLLER}>
          <table className={TABLE} aria-label={`History of ${branch}`}>
            <TableHead columns={COLUMNS} />
            <tbody>
              {entries.map((entry) => {
                const range =
                  open === entry.commit
                    ? revertRange(entries, entry.commit)
                    : null;
                return (
                  <EntryRows
                    key={entry.commit}
                    projectId={projectId}
                    branch={branch}
                    entry={entry}
                    mark={historyRowMark(entry, head)}
                    highlighted={highlight === entry.commit}
                    opened={range !== null}
                    disabled={disabled}
                    onShowRow={setHighlight}
                    onToggle={() => {
                      setLast(null);
                      setHighlight(null);
                      setOpen((current) =>
                        current === entry.commit ? null : entry.commit,
                      );
                    }}
                    confirm={
                      range !== null &&
                      head !== undefined && (
                        <RevertConfirm
                          projectId={projectId}
                          branch={branch}
                          target={entry}
                          range={range}
                          expectedHead={head.commit}
                          formId={formId}
                          onBusy={onBusy}
                          onCancel={() => {
                            setOpen(null);
                          }}
                          onReverted={(result) => {
                            setOpen(null);
                            setLast({ branch, to: entry.commit, result });
                            setHighlight(result.commit);
                          }}
                        />
                      )
                    }
                  />
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {history.isFetchNextPageError && (
        <Alert kind="error">Could not load older commits.</Alert>
      )}

      {history.hasNextPage && (
        <SubmitButton
          type="button"
          variant="ghost"
          loading={history.isFetchingNextPage}
          icon={Icon.expand}
          onClick={() => {
            void history.fetchNextPage();
          }}
        >
          Load older
        </SubmitButton>
      )}
    </div>
  );
}

interface EntryRowsProps {
  projectId: string;
  branch: string;
  entry: HistoryEntry;
  mark: HistoryRowMark;
  /** Drawn highlighted and scrolled into view: a new revert, or a link's target. */
  highlighted: boolean;
  opened: boolean;
  disabled: boolean;
  /** Highlight and scroll to the row of `commit`. */
  onShowRow: (commit: string) => void;
  onToggle: () => void;
  confirm: ReactNode;
}

function EntryRows({
  projectId,
  branch,
  entry,
  mark,
  highlighted,
  opened,
  disabled,
  onShowRow,
  onToggle,
  confirm,
}: EntryRowsProps) {
  const short = shortSha(entry.commit);
  const row = useRef<HTMLTableRowElement>(null);
  useEffect(() => {
    if (highlighted) {
      row.current?.scrollIntoView({ block: "nearest" });
    }
  }, [highlighted]);

  const undone = mark.kind === "undone";
  const rowClass = [
    opened ? "border-0" : ROW,
    undone ? "opacity-60" : "",
    highlighted ? "bg-console-accent/10" : "",
  ].join(" ");
  return (
    <>
      <tr
        ref={row}
        id={historyRowId(entry.commit)}
        className={rowClass}
        data-highlighted={highlighted || undefined}
      >
        <td
          className={`${CELL_TOP} text-console-muted font-mono text-xs whitespace-nowrap`}
          title={entry.commit}
        >
          {short}
        </td>
        <td className={`${CELL_TOP} text-console-text min-w-0 text-xs`}>
          <span
            className={`line-clamp-2 break-words ${undone ? "decoration-console-muted line-through" : ""}`}
          >
            {entry.subject}
          </span>
          <span className="text-console-muted block">{entry.author_name}</span>
        </td>
        <td
          className={`${CELL_TOP} text-console-muted hidden font-mono text-xs md:table-cell`}
        >
          <RequestedByCell entry={entry} />
        </td>
        <td className={`${CELL_TOP} text-xs`}>
          <Attribution projectId={projectId} entry={entry} />
        </td>
        <td
          className={`${CELL_TOP} text-console-muted hidden text-xs whitespace-nowrap sm:table-cell`}
          title={formatDateTime(entry.committed_at)}
        >
          {formatRelative(entry.committed_at)}
        </td>
        <td className={`${CELL_TOP} pr-0 text-right whitespace-nowrap`}>
          {mark.kind === "undone" && (
            <a
              href={`#${historyRowId(mark.by)}`}
              title={`Undone by the revert ${mark.by}`}
              onClick={(event) => {
                event.preventDefault();
                onShowRow(mark.by);
              }}
              className={`text-console-muted hover:text-console-text font-mono text-xs hover:underline ${TAP_INLINE}`}
            >
              undone by {shortSha(mark.by)}
            </a>
          )}
          {mark.kind === "current" && (
            <span
              title={`${branch} already holds this commit's content`}
              className="text-console-muted font-mono text-xs"
            >
              current content
            </span>
          )}
          {mark.kind === "revertible" && (
            <button
              type="button"
              aria-expanded={opened}
              disabled={disabled}
              title={`Revert ${branch} to ${short}`}
              onClick={onToggle}
              className={`inline-flex items-center gap-1.5 font-mono text-xs disabled:opacity-50 ${
                opened
                  ? "text-console-accent"
                  : "text-console-muted hover:text-console-text"
              }`}
            >
              <Icon.revert aria-hidden="true" className={ICON_CLASS} />
              Revert to here
            </button>
          )}
        </td>
      </tr>
      {opened && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className={SPAN_CELL_ROOMY}>
            {confirm}
          </td>
        </tr>
      )}
    </>
  );
}

/** The `Requested-By` trailer as a person, a session or the system. */
function RequestedByCell({ entry }: { entry: HistoryEntry }) {
  const requested = parseRequestedBy(entry.requested_by);
  const username = useUsername(
    requested?.kind === "user" ? requested.id : null,
  );

  switch (requested?.kind) {
    case undefined:
      return <>{PLACEHOLDER}</>;
    case "user":
      return <span title="A person">{username}</span>;
    case "session": {
      const title = entry.sessions.find((s) => s.id === requested.id)?.title;
      return (
        <Link
          to={`/sessions/${requested.id}`}
          className="hover:text-console-text hover:underline"
          title="A session"
        >
          {title ?? `session ${requested.id.slice(0, 8)}`}
        </Link>
      );
    }
    case "system":
      return <span title="The orchestrator itself">system</span>;
    case "other":
      return <>{requested.text}</>;
  }
}

/** The tasks and sessions an entry is attributed to, as links. */
function Attribution({
  projectId,
  entry,
}: {
  projectId: string;
  entry: HistoryEntry;
}) {
  if (entry.tasks.length === 0 && entry.sessions.length === 0) {
    return <span className="text-console-muted">{PLACEHOLDER}</span>;
  }
  return (
    <span className="flex flex-wrap gap-x-2 gap-y-0.5">
      {entry.tasks.map((task) => (
        <Link
          key={task.id}
          to={taskPath(projectId, task.number)}
          title={task.title}
          className="text-console-accent font-mono hover:underline"
        >
          #{task.number}
        </Link>
      ))}
      {entry.sessions.map((session) => (
        <Link
          key={session.id}
          to={`/sessions/${session.id}`}
          className="text-console-muted hover:text-console-text font-mono hover:underline"
        >
          {session.title ?? session.id.slice(0, 8)}
        </Link>
      ))}
    </span>
  );
}
