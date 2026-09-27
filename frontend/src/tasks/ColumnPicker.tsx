// The board's column picker below `sm` (`SPEC.md`, "Frontend", "Mobile
// layout"): a phone shows one column at a time, so a row of chips above the
// strip says how many there are, what each is called and holds, and which one
// is in view, and a tap scrolls the strip to that column.
//
// The row is only ever a way of moving the strip: it never scrolls anything on
// its own but itself. Keeping the marked chip in view is done by setting the
// row's own `scrollLeft`, never by `scrollIntoView`, which would also scroll
// the page — and under an open drawer, the page is not the user's to move.

import { useEffect, useRef } from "react";

import { TAP } from "../components/fieldStyles";
import { UNKNOWN_COLUMN } from "./taskStore";
import type { TaskColumn } from "./taskStore";
import { COLUMN_KEY_ATTRIBUTE } from "./useColumnInView";

export interface ColumnPickerProps {
  columns: readonly TaskColumn[];
  /** The key of the column in view, marked `aria-current`. */
  active: string | undefined;
  onPick: (key: string) => void;
}

export function ColumnPicker({ columns, active, onPick }: ColumnPickerProps) {
  const row = useRef<HTMLDivElement>(null);

  // The marked chip stays in the row's view as the strip is swiped past
  // columns whose chips are off the row's edge.
  useEffect(() => {
    const element = row.current;
    if (element === null || active === undefined) return;
    const chip = Array.from(element.children).find(
      (child) => child.getAttribute(COLUMN_KEY_ATTRIBUTE) === active,
    );
    if (!(chip instanceof HTMLElement)) return;
    const left = chip.offsetLeft;
    const right = left + chip.offsetWidth;
    if (left < element.scrollLeft) {
      element.scrollLeft = left;
    } else if (right > element.scrollLeft + element.clientWidth) {
      element.scrollLeft = right - element.clientWidth;
    }
  }, [active]);

  return (
    <nav aria-label="Board columns">
      <div ref={row} className="relative flex gap-1.5 overflow-x-auto pb-1">
        {columns.map((column) => {
          const current = column.key === active;
          return (
            <button
              key={column.key}
              type="button"
              // `COLUMN_KEY_ATTRIBUTE`, as the column itself carries it.
              data-column-key={column.key}
              // The two spans would run together as `backlog3` when read out.
              aria-label={`${column.name} (${String(column.tasks.length)})`}
              aria-current={current ? "true" : undefined}
              onClick={() => {
                onPick(column.key);
              }}
              className={`inline-flex shrink-0 items-center gap-1.5 rounded border px-2 py-1 font-mono text-xs ${TAP} ${
                current
                  ? "border-console-accent/70 bg-console-raised text-console-text"
                  : "border-console-border text-console-muted hover:text-console-text"
              }`}
            >
              <span
                className={
                  column.name === UNKNOWN_COLUMN
                    ? "text-console-muted"
                    : undefined
                }
              >
                {column.name}
              </span>
              <span className="text-console-muted">{column.tasks.length}</span>
            </button>
          );
        })}
      </div>
    </nav>
  );
}
