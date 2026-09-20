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
      <span className="whitespace-pre">{line.text}</span>
    </div>
  );
}

function HalfRow({ line }: { line?: DiffLine }) {
  if (!line) {
    return <div className="bg-console-raised/40 flex min-h-[1.25rem] flex-1" />;
  }
  return (
    <div className={`flex min-w-0 flex-1 ${TINT[line.type]}`}>
      <LineNo value={line.type === "add" ? line.newNo : line.oldNo} />
      <span className="overflow-x-auto whitespace-pre">{line.text}</span>
    </div>
  );
}

/** Pairs each run of deletions with the run of additions that replaced it. */
function pairs(lines: DiffLine[]): { left?: DiffLine; right?: DiffLine }[] {
  const rows: { left?: DiffLine; right?: DiffLine }[] = [];
  let i = 0;
  while (i < lines.length) {
    if (lines[i].type === "context") {
      rows.push({ left: lines[i], right: lines[i] });
      i += 1;
      continue;
    }
    const dels: DiffLine[] = [];
    while (i < lines.length && lines[i].type === "del") {
      dels.push(lines[i]);
      i += 1;
    }
    const adds: DiffLine[] = [];
    while (i < lines.length && lines[i].type === "add") {
      adds.push(lines[i]);
      i += 1;
    }
    for (let k = 0; k < Math.max(dels.length, adds.length); k += 1) {
      rows.push({ left: dels[k], right: adds[k] });
    }
  }
  return rows;
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
      <div className="overflow-x-auto px-1 py-1 font-mono text-xs">
        {split
          ? pairs(lines).map((row, index) => (
              <div key={index} className="flex gap-2">
                <HalfRow line={row.left} />
                <HalfRow line={row.right} />
              </div>
            ))
          : lines.map((line, index) => <UnifiedRow key={index} line={line} />)}
      </div>
    </div>
  );
}
