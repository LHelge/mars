// The one table idiom of this console, in one module.
//
// Every list in Mars is a dense table — the operator scans a column, not a
// page of cards — and until this module existed each of them carried its own
// copy of the four class strings. The copies drifted: `py-2` against `py-1.5`,
// `align-top` against `align-middle`, a hover tint on one table and not the
// next. There is one row height here and two vertical alignments, and a table
// that wants something else says so in the extra classes it appends.
//
// Kept to class constants and a type on purpose: `DashboardPage` and
// `ProjectsPage` are on the first-paint path (`ARCHITECTURE.md`, "Frontend
// architecture", Barrels and the first-paint path), so whatever they import
// lands in the entry chunk.

/** The table element itself. */
export const TABLE = "w-full border-collapse text-sm";

/** A `<thead>` that stays put while a long table scrolls under it. */
export const THEAD_STICKY = "bg-console-bg sticky top-0 z-10";

/** The box a long table scrolls inside, under a sticky header. */
export const SCROLLER = "max-h-96 overflow-y-auto";

/** The box a wide table scrolls sideways in on a narrow screen. */
export const X_SCROLLER = "overflow-x-auto";

/** A header cell. */
export const HEAD =
  "text-console-muted py-1.5 pr-3 text-left text-xs font-normal";

/** A body cell whose content is one line. */
export const CELL = "py-1.5 pr-3 align-middle";

/**
 * A body cell whose content stacks — a status and the reason under it, a name
 * with a badge beneath. Its neighbours line up with its first line.
 */
export const CELL_TOP = "py-1.5 pr-3 align-top";

/** A body row. */
export const ROW = "border-console-border/60 border-b last:border-b-0";

/** A body row whose whole width is one link, so the row answers the pointer. */
export const ROW_HOVER = `${ROW} hover:bg-console-raised/60`;

/**
 * The cell of a row that spans the table: an expanded panel, a confirmation,
 * the answer to something pressed on the row above it.
 */
export const SPAN_CELL = "bg-console-surface/60 px-3 py-2";

/** The same, for a panel that holds a form rather than one line. */
export const SPAN_CELL_ROOMY = "bg-console-surface/60 px-3 py-3";

/** A spanning cell with no tint, for an alert that carries its own border. */
export const SPAN_CELL_BARE = "px-0 py-2";

/**
 * One column of a table.
 *
 * The array of these is the table's header *and* its width: a row that spans
 * the table reads `columns.length` rather than repeating a number that lives
 * in another file (`SecretsManager` defines the header its `SecretRow`s span).
 */
export interface TableColumn {
  /** The header text, and the accessible name of the column. */
  label: string;
  /** Extra classes on the `<th>`: a responsive `hidden`, an alignment. */
  className?: string;
  /** The label is for screen readers only — an actions column, typically. */
  srOnly?: boolean;
}
