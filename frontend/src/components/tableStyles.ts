// The one table idiom of this console, in one module.
//
// Every list in Mars is a dense table — the operator scans a column, not a
// page of cards — and until this module existed each of them carried its own
// copy of the four class strings. The copies drifted: `py-2` against `py-1.5`,
// `align-top` against `align-middle`, a hover tint on one table and not the
// next. There is one row height here and two vertical alignments, and a table
// that wants something else says so in the extra classes it appends.
//
// Three rules every table follows, and they live here rather than in the
// tables (`SPEC.md`, "Frontend", "Mobile layout": no route scrolls sideways):
//
// - A table is always inside a scroller: `X_SCROLLER`, or `SCROLLER` for a
//   long one under a sticky header. A table the viewport cannot hold scrolls
//   inside its own box and never pushes the page sideways.
// - A cell that truncates is capped by its column. Automatic table layout
//   ignores a `max-w` on a `<td>`, so a `truncate` inside an uncapped cell
//   widens the column to the whole string. Either the truncating block carries
//   the width cap itself (`block max-w-[44ch] truncate`), or the cell takes the
//   rest of the row as `w-full max-w-0`, which is the only width a `<td>` is
//   held to.
// - The phone column set is what identifies the row, plus its state and its
//   one action. Everything else is hidden below a breakpoint with the
//   column's responsive `hidden` class, on the `TableColumn` and on the row's
//   cell alike; hiding is CSS only, so `columns.length` — what a spanning row
//   reads — never changes with the viewport.
//
// Kept to class constants and a type on purpose: `DashboardPage` and
// `ProjectsPage` are on the first-paint path (`ARCHITECTURE.md`, "Frontend
// architecture", Barrels and the first-paint path), so whatever they import
// lands in the entry chunk.

/** The table element itself. */
export const TABLE = "w-full border-collapse text-sm";

/** A `<thead>` that stays put while a long table scrolls under it. */
export const THEAD_STICKY = "bg-console-bg sticky top-0 z-10";

/**
 * The box a long table scrolls inside, under a sticky header. The 384 px cap
 * is `sm` and up only: on a phone a nested vertical scroll area catches the
 * finger that meant to scroll the page, so there the table runs the page's
 * length. It scrolls sideways too, explicitly, like `X_SCROLLER`.
 */
export const SCROLLER = "overflow-x-auto overflow-y-auto sm:max-h-96";

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
  /**
   * A shorter header shown below `sm`, where a long label would set the
   * column's width. `label` stays the accessible name at every width.
   */
  short?: string;
}
