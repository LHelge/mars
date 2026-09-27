// A table's header, rendered from the column definition the table's spanning
// rows also count (`tableStyles.ts`, `TableColumn`).

import { HEAD, THEAD_STICKY, type TableColumn } from "./tableStyles";

export interface TableHeadProps {
  columns: readonly TableColumn[];
  /** Keep the header visible while a long table scrolls under it. */
  sticky?: boolean;
}

export function TableHead({ columns, sticky = false }: TableHeadProps) {
  return (
    <thead className={sticky ? THEAD_STICKY : undefined}>
      <tr className="border-console-border border-b">
        {columns.map((column) => (
          <th
            key={column.label}
            scope="col"
            className={
              column.className === undefined
                ? HEAD
                : `${HEAD} ${column.className}`
            }
          >
            {column.srOnly ? (
              <span className="sr-only">{column.label}</span>
            ) : column.short === undefined ? (
              column.label
            ) : (
              // The short text is what a phone shows; the full label stays
              // the name a screen reader hears, at every width.
              <>
                <span aria-hidden="true" className="sm:hidden">
                  {column.short}
                </span>
                <span className="max-sm:sr-only">{column.label}</span>
              </>
            )}
          </th>
        ))}
      </tr>
    </thead>
  );
}
