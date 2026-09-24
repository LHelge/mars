// The integration heads of the project mirror — `main` and any other branch
// Mars owns — each with its commit and the commit its upstream-tracking ref
// was at on the last fetch (`help/branches.md`, "Three kinds of branch").
//
// The default branch is marked, because it is what a launch starts from and
// what every merge form suggests. The trailing column is the row's actions:
// `Push…`, because merged work waits on a head until someone pushes it
// (`README.md`, "Operating notes"). Like a session row's actions it opens its
// form in place, under the row.

import type { ReactNode } from "react";
import { PLACEHOLDER, shortSha } from "../../utils/format";
import { Icon, ICON_CLASS } from "../icons";
import { TableHead } from "../TableHead";
import { CELL, ROW, TABLE, type TableColumn } from "../tableStyles";
import type { IntegrationHead } from "./integrationHeads";

const COLUMNS: readonly TableColumn[] = [
  { label: "Head" },
  { label: "Commit" },
  { label: "Upstream" },
  { label: "Actions", className: "pr-0 text-right", srOnly: true },
];

export interface IntegrationHeadTableProps {
  heads: IntegrationHead[];
  /** The head whose push form is open, by name. */
  open: string | null;
  onToggle: (name: string) => void;
  /** The open head's form, rendered under its row. */
  renderForm: (head: IntegrationHead) => ReactNode;
  disabled: boolean;
}

export function IntegrationHeadTable({
  heads,
  open,
  onToggle,
  renderForm,
  disabled,
}: IntegrationHeadTableProps) {
  return (
    <table className={TABLE}>
      <TableHead columns={COLUMNS} />
      <tbody>
        {heads.map((head) => (
          <HeadRows
            key={head.name}
            head={head}
            opened={open === head.name}
            onToggle={onToggle}
            renderForm={renderForm}
            disabled={disabled}
          />
        ))}
      </tbody>
    </table>
  );
}

interface HeadRowsProps {
  head: IntegrationHead;
  opened: boolean;
  onToggle: (name: string) => void;
  renderForm: (head: IntegrationHead) => ReactNode;
  disabled: boolean;
}

function HeadRows({
  head,
  opened,
  onToggle,
  renderForm,
  disabled,
}: HeadRowsProps) {
  return (
    <>
      <tr className={opened ? "border-0" : ROW}>
        <td className={`${CELL} min-w-0`}>
          <span className="flex flex-wrap items-center gap-2">
            <span className="text-console-text font-mono text-xs">
              {head.name}
            </span>
            {head.isDefault && (
              <span
                className="border-console-accent/60 text-console-accent inline-flex items-center rounded border px-1.5 py-0.5 font-mono text-xs whitespace-nowrap"
                title="The project's default branch: what a launch starts from and a merge suggests"
              >
                default
              </span>
            )}
          </span>
        </td>
        <td className={`${CELL} text-console-muted font-mono text-xs`}>
          {shortSha(head.commit)}
        </td>
        <td
          className={`${CELL} text-console-muted font-mono text-xs whitespace-nowrap`}
        >
          {head.upstream === null ? (
            <span title={`The remote had no ${head.name} at the last fetch`}>
              {PLACEHOLDER}
            </span>
          ) : (
            <span title={`origin/${head.name} at the last fetch`}>
              origin/{head.name} {shortSha(head.upstream)}
              {head.upstream === head.commit && " · same commit"}
            </span>
          )}
        </td>
        <td className={`${CELL} pr-0 text-right whitespace-nowrap`}>
          <button
            type="button"
            aria-expanded={opened}
            disabled={disabled}
            title={`Push ${head.name} to a branch on the remote`}
            onClick={() => {
              onToggle(head.name);
            }}
            className={`ml-2 inline-flex items-center gap-1.5 font-mono text-xs disabled:opacity-50 ${
              opened
                ? "text-console-accent"
                : "text-console-muted hover:text-console-text"
            }`}
          >
            <Icon.push aria-hidden="true" className={ICON_CLASS} />
            Push…
          </button>
        </td>
      </tr>

      {opened && (
        <tr className={ROW}>
          <td colSpan={COLUMNS.length} className="pt-1 pb-3">
            <div className="border-console-border bg-console-surface rounded border px-3 py-3">
              {renderForm(head)}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
