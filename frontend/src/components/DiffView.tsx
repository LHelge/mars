// A line diff, drawn (`SPEC.md`, "Transcript rendering" and "Changes panel").
// Edit tools and the Changes panel both produce `DiffLine[]`, so both draw it
// through this one component.
//
// Unified is the default because it is the form a reviewer reads in a narrow
// column; side by side is one click away for a rewrite, where the two versions
// are easier to compare than to interleave.

import { useState } from "react";

import type { DiffLine } from "../utils/diff";

const TINT: Record<DiffLine["type"], string> = {
  add: "bg-state-running/10",
  del: "bg-state-failed/10",
  context: "",
};

const MARK: Record<DiffLine["type"], string> = {
  add: "+",
  del: "-",
  context: " ",
};

/**
 * What a screen reader is told a row is. The `+`/`-` marker is decorative —
 * it is announced as punctuation or not at all — and in side-by-side mode
 * there is no marker at all, only colour and which pane the line is in, so
 * every changed row says so in words. A context line says nothing: it is the
 * unchanged majority and naming it would drown the two that matter.
 */
const SR_LABEL: Record<DiffLine["type"], string | null> = {
  add: "added",
  del: "removed",
  context: null,
};

function Marker({ type }: { type: DiffLine["type"] }) {
  const label = SR_LABEL[type];
  return label === null ? null : <span className="sr-only">{label} </span>;
}

function LineNo({ value }: { value?: number }) {
  return (
    <span className="text-console-muted w-10 shrink-0 pr-2 text-right tabular-nums select-none">
      {value ?? ""}
    </span>
  );
}

function UnifiedRow({ line }: { line: DiffLine }) {
  return (
    <div className={`flex ${TINT[line.type]}`}>
      <LineNo value={line.oldNo} />
      <LineNo value={line.newNo} />
      <span aria-hidden="true" className="text-console-muted w-3 shrink-0">
        {MARK[line.type]}
      </span>
      <Marker type={line.type} />
      <span className="whitespace-pre">{line.text}</span>
    </div>
  );
}

function HalfRow({ line }: { line?: DiffLine }) {
  if (!line) {
    return <div className="bg-console-raised/40 flex min-h-[1.25rem]" />;
  }
  return (
    <div className={`flex min-h-[1.25rem] ${TINT[line.type]}`}>
      <LineNo value={line.type === "add" ? line.newNo : line.oldNo} />
      <Marker type={line.type} />
      <span className="whitespace-pre">{line.text}</span>
    </div>
  );
}

/** Pairs each run of deletions with the run of additions that replaced it. */
function pairs(lines: DiffLine[]): { left?: DiffLine; right?: DiffLine }[] {
  const rows: { left?: DiffLine; right?: DiffLine }[] = [];
  let i = 0;
  while (i < lines.length) {
    // One read per step, so the loop bound and the value it guards are the
    // same expression: `i < lines.length` is what makes each of these present.
    const line = lines[i];
    if (line === undefined) {
      break;
    }
    if (line.type === "context") {
      rows.push({ left: line, right: line });
      i += 1;
      continue;
    }
    const dels: DiffLine[] = [];
    for (let del = lines[i]; del?.type === "del"; del = lines[i]) {
      dels.push(del);
      i += 1;
    }
    const adds: DiffLine[] = [];
    for (let add = lines[i]; add?.type === "add"; add = lines[i]) {
      adds.push(add);
      i += 1;
    }
    for (let k = 0; k < Math.max(dels.length, adds.length); k += 1) {
      rows.push({ left: dels[k], right: adds[k] });
    }
  }
  return rows;
}

function SplitPanes({
  rows,
}: {
  rows: { left?: DiffLine; right?: DiffLine }[];
}) {
  return (
    <div className="flex gap-2 px-1 py-1 font-mono text-xs">
      <div className="min-w-0 flex-1 overflow-x-auto">
        {rows.map((row, index) => (
          <HalfRow key={index} line={row.left} />
        ))}
      </div>
      <div className="min-w-0 flex-1 overflow-x-auto">
        {rows.map((row, index) => (
          <HalfRow key={index} line={row.right} />
        ))}
      </div>
    </div>
  );
}

export interface DiffViewProps {
  lines: DiffLine[];
  /** Shown as the block's header; omitted when the caller has its own. */
  path?: string;
  /** A line above the diff, e.g. that the alignment was skipped. */
  notice?: string;
}

export function DiffView({ lines, path, notice }: DiffViewProps) {
  const [split, setSplit] = useState(false);
  const added = lines.filter((line) => line.type === "add").length;
  const removed = lines.filter((line) => line.type === "del").length;

  return (
    <div className="border-console-border bg-console-bg overflow-hidden rounded border">
      <div className="border-console-border flex items-center gap-3 border-b px-3 py-1">
        {path !== undefined && (
          <span className="text-console-text truncate font-mono text-xs">
            {path}
          </span>
        )}
        <span className="text-console-muted font-mono text-xs">
          <span className="text-state-running">+{added}</span>{" "}
          <span className="text-state-failed">−{removed}</span>
        </span>
        <button
          type="button"
          onClick={() => setSplit((value) => !value)}
          aria-pressed={split}
          className="text-console-accent ml-auto text-xs underline underline-offset-2"
        >
          {split ? "Unified" : "Side by side"}
        </button>
      </div>
      {notice !== undefined && (
        <p className="text-console-muted border-console-border border-b px-3 py-1 text-xs">
          {notice}
        </p>
      )}
      {split ? (
        // One scrollbar per pane, not one per row: the overflow belongs to
        // the two columns, so a long line scrolls its whole side and the row
        // numbers of the two versions stay level.
        <SplitPanes rows={pairs(lines)} />
      ) : (
        <div className="overflow-x-auto px-1 py-1 font-mono text-xs">
          {lines.map((line, index) => (
            <UnifiedRow key={index} line={line} />
          ))}
        </div>
      )}
    </div>
  );
}
