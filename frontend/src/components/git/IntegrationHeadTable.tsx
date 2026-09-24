// The integration heads of the project mirror — `main` and any other branch
// Mars owns — each with its commit and the commit its upstream-tracking ref
// was at on the last fetch (`help/branches.md`, "Three kinds of branch").
//
// The default branch is marked, because it is what a launch starts from and
// what every merge form suggests. The trailing column is the row's actions,
// kept while it is still empty so an action on a head — pushing it — has a
// place to land without the table moving.

import { PLACEHOLDER, shortSha } from "../../utils/format";
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
}

export function IntegrationHeadTable({ heads }: IntegrationHeadTableProps) {
  return (
    <table className={TABLE}>
      <TableHead columns={COLUMNS} />
      <tbody>
        {heads.map((head) => (
          <tr key={head.name} className={ROW}>
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
                <span
                  title={`The remote had no ${head.name} at the last fetch`}
                >
                  {PLACEHOLDER}
                </span>
              ) : (
                <span title={`origin/${head.name} at the last fetch`}>
                  origin/{head.name} {shortSha(head.upstream)}
                  {head.upstream === head.commit && " · same commit"}
                </span>
              )}
            </td>
            {/* No action on a head yet; the cell holds its place. */}
            <td className={`${CELL} pr-0 text-right whitespace-nowrap`} />
          </tr>
        ))}
      </tbody>
    </table>
  );
}
