// The session refs in the project mirror, with how far each one has run ahead
// of the integration head and how far it has fallen behind
// (`SPEC.md`, "Git": `GET /projects/{pid}/git/session-branches`).
//
// The pair `+n / −m` is the whole reason to look at this table, so it is the
// only colour in it: ahead is work waiting to be merged, behind is work the
// branch has not seen yet. Everything else is an identifier and stays quiet.
//
// A row's three actions open their form in place, under the row, because the
// decision being made is about that branch and a modal would hide it.

import type { ReactNode } from "react";
import { Link } from "react-router";
import type { SessionBranch } from "../../types";
import { formatRelative, shortSha } from "../../utils/format";
import { Icon, ICON_CLASS } from "../icons";
import type { IconComponent } from "../icons";
import { TableHead } from "../TableHead";
import { CELL, ROW, TABLE, X_SCROLLER, type TableColumn } from "../tableStyles";
import { TAP_INLINE } from "../fieldStyles";

/** Which form a row has open. */
export type RowAction = "merge" | "rebase" | "push";

export interface OpenRow {
  sessionId: string;
  action: RowAction;
}

export interface SessionBranchTableProps {
  rows: SessionBranch[];
  /** Session id to title, when the project's session list is loaded. */
  titles: Map<string, string>;
  /** The compare link each row kept from its last push. */
  compareLinks: Record<string, { commit: string; url: string }>;
  open: OpenRow | null;
  onToggle: (sessionId: string, action: RowAction) => void;
  /** The open row's form, rendered under it. */
  renderForm: (row: SessionBranch) => ReactNode;
  disabled: boolean;
}

const COLUMNS: readonly TableColumn[] = [
  { label: "Session" },
  { label: "Ref", className: "hidden lg:table-cell" },
  { label: "Commit", className: "hidden sm:table-cell" },
  { label: "Ahead / behind" },
  { label: "Updated", className: "hidden sm:table-cell" },
  { label: "Actions", className: "pr-0 text-right", srOnly: true },
];

const ACTIONS: { action: RowAction; label: string; icon: IconComponent }[] = [
  { action: "merge", label: "Merge into…", icon: Icon.merge },
  { action: "rebase", label: "Rebase onto…", icon: Icon.rebase },
  { action: "push", label: "Push…", icon: Icon.push },
];

export function SessionBranchTable({
  rows,
  titles,
  compareLinks,
  open,
  onToggle,
  renderForm,
  disabled,
}: SessionBranchTableProps) {
  return (
    <div className={X_SCROLLER}>
      <table className={TABLE}>
        <TableHead columns={COLUMNS} />
        <tbody>
          {rows.map((row) => {
            const compare = compareLinks[row.session_id];
            const title = titles.get(row.session_id);
            const opened =
              open?.sessionId === row.session_id ? open.action : null;

            return (
              <BranchRows
                key={row.session_id}
                row={row}
                title={title}
                compareUrl={
                  compare !== undefined && compare.commit === row.commit
                    ? compare.url
                    : null
                }
                opened={opened}
                onToggle={onToggle}
                renderForm={renderForm}
                disabled={disabled}
              />
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

interface BranchRowsProps {
  row: SessionBranch;
  title: string | undefined;
  compareUrl: string | null;
  opened: RowAction | null;
  onToggle: (sessionId: string, action: RowAction) => void;
  renderForm: (row: SessionBranch) => ReactNode;
  disabled: boolean;
}

function BranchRows({
  row,
  title,
  compareUrl,
  opened,
  onToggle,
  renderForm,
  disabled,
}: BranchRowsProps) {
  return (
    <>
      <tr className={opened === null ? ROW : "border-0"}>
        <td className={`${CELL} min-w-0`}>
          <Link
            to={`/sessions/${row.session_id}`}
            className="text-console-text hover:text-console-accent"
          >
            {title ?? (
              <span className="font-mono text-xs">
                {row.session_id.slice(0, 8)}
              </span>
            )}
          </Link>
          {compareUrl !== null && (
            <a
              href={compareUrl}
              target="_blank"
              rel="noreferrer noopener"
              className={`text-console-accent ml-2 font-mono text-xs underline ${TAP_INLINE}`}
            >
              compare
            </a>
          )}
        </td>
        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs lg:table-cell`}
        >
          {row.ref}
        </td>
        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs sm:table-cell`}
        >
          {shortSha(row.commit)}
        </td>
        <td className={`${CELL} font-mono text-xs whitespace-nowrap`}>
          {/* The counts are for the eye, each titled; a screen reader gets
              one sentence, and the base they count against is text under
              them, since no other column shows it. */}
          <span aria-hidden="true">
            <span
              className={
                row.ahead > 0 ? "text-state-running" : "text-console-muted"
              }
              title={`${String(row.ahead)} ahead of ${row.base}`}
            >
              +{row.ahead}
            </span>
            <span className="text-console-muted"> / </span>
            <span
              className={
                row.behind > 0 ? "text-state-parked" : "text-console-muted"
              }
              title={`${String(row.behind)} behind ${row.base}`}
            >
              −{row.behind}
            </span>
            <span className="text-console-muted block text-[0.6875rem]">
              vs {row.base}
            </span>
          </span>
          <span className="sr-only">
            {row.ahead} ahead of {row.base}, {row.behind} behind
          </span>
        </td>
        <td
          className={`${CELL} text-console-muted hidden font-mono text-xs whitespace-nowrap sm:table-cell`}
        >
          {formatRelative(row.updated_at)}
        </td>
        <td className={`${CELL} pr-0`}>
          {/* Three text buttons do not fit a phone's row on one line: they
              wrap, right-aligned, rather than set the table's width. */}
          <div className="flex flex-wrap justify-end gap-x-2 gap-y-1">
            {ACTIONS.map((entry) => (
              <button
                key={entry.action}
                type="button"
                aria-expanded={opened === entry.action}
                disabled={disabled}
                onClick={() => {
                  onToggle(row.session_id, entry.action);
                }}
                className={`inline-flex items-center gap-1.5 font-mono text-xs whitespace-nowrap disabled:opacity-50 ${TAP_INLINE} ${
                  opened === entry.action
                    ? "text-console-accent"
                    : "text-console-muted hover:text-console-text"
                }`}
              >
                <entry.icon aria-hidden="true" className={ICON_CLASS} />
                {entry.label}
              </button>
            ))}
          </div>
        </td>
      </tr>

      {opened !== null && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className="pt-1 pb-3">
            <div className="border-console-border bg-console-surface rounded border px-3 py-3">
              {renderForm(row)}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
