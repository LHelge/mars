// A whole `Diff`, drawn (`SPEC.md`, "Frontend", "Changes panel"): the file
// list with its counts and truncation notice, then the patch itself, one
// collapsible block per file, with a click in the list scrolling to the block.
//
// The session's Changes panel and the task drawer's hand-off diff show the
// same thing about two different targets — a live session branch and the
// immutable commit a hand-off retained — so the drawing lives here and each
// caller only decides what to fetch and what to say above it.
//
// A patch is drawn synchronously, five elements to the line, so the size of
// the patch is the size of the mount: the endpoint's own bound is 1 MiB
// (`SPEC.md`, "Git"), which is tens of thousands of lines. Two bounds keep
// that off the main thread — one file that is large on its own, and a patch
// whose files are only large together — and both are opt-in rather than
// lossy: nothing is hidden, everything is one click away.

import { useMemo, useRef, useState } from "react";

import type { Diff } from "../../types";
import type { PatchFile } from "../../utils/diff";
import { parseUnifiedPatch } from "../../utils/diff";
import { DiffView } from "../DiffView";
import { ChangesFileList } from "./ChangesFileList";

/** Above this many changed lines a file opens collapsed. */
const COLLAPSE_CHANGED_LINES = 500;

/**
 * Above this many changed lines in the whole patch every file opens
 * collapsed, whatever its own size: a hundred files of two hundred lines each
 * is the same number of rows as one file of twenty thousand, and the per-file
 * rule alone lets all hundred through.
 */
const RENDER_BUDGET_CHANGED_LINES = 2000;

/** The changed (non-context) lines of one file. */
function changedLines(file: PatchFile): number {
  return file.hunks.reduce(
    (sum, hunk) =>
      sum + hunk.lines.filter((line) => line.type !== "context").length,
    0,
  );
}

export interface DiffBodyProps {
  diff: Diff;
}

export function DiffBody({ diff }: DiffBodyProps) {
  const patch = diff.patch;
  const files = useMemo(() => parseUnifiedPatch(patch), [patch]);
  const binaryPaths = useMemo(
    () => new Set(files.filter((file) => file.binary).map((file) => file.path)),
    [files],
  );
  const changed = useMemo(() => files.map(changedLines), [files]);
  const total = changed.reduce((sum, count) => sum + count, 0);
  const overBudget = total > RENDER_BUDGET_CHANGED_LINES;

  // One ref for the whole patch, with each block carrying its own path: an
  // inline ref callback per file would be a new function every render, so
  // React would delete and re-set every entry of a map on every render and a
  // memoised block would never be able to skip one.
  const blocks = useRef<HTMLDivElement | null>(null);
  const scrollToFile = (path: string) => {
    blocks.current
      ?.querySelector(`[data-path="${CSS.escape(path)}"]`)
      ?.scrollIntoView({ block: "start" });
  };

  return (
    <>
      <ChangesFileList
        diff={diff}
        binaryPaths={binaryPaths}
        onSelect={scrollToFile}
      />
      {overBudget && (
        <p className="text-state-parked font-mono text-xs">
          {total} changed lines across {files.length}{" "}
          {files.length === 1 ? "file" : "files"}; every file starts collapsed
        </p>
      )}
      <div ref={blocks} className="space-y-3">
        {files.map((file, index) => (
          <PatchFileBlock
            key={file.path}
            file={file}
            changed={changed[index]}
            startCollapsed={overBudget}
          />
        ))}
      </div>
    </>
  );
}

interface PatchFileBlockProps {
  file: PatchFile;
  changed: number;
  /** The whole patch is past its budget: open collapsed whatever the size. */
  startCollapsed: boolean;
}

/** One file of the patch: a header that collapses it, and its hunks. */
function PatchFileBlock({
  file,
  changed,
  startCollapsed,
}: PatchFileBlockProps) {
  const [expanded, setExpanded] = useState(
    !startCollapsed && changed <= COLLAPSE_CHANGED_LINES,
  );

  return (
    <section
      data-path={file.path}
      className="border-console-border rounded border"
    >
      <button
        type="button"
        onClick={() => {
          setExpanded((value) => !value);
        }}
        aria-expanded={expanded}
        className="flex w-full items-baseline gap-2 px-2 py-1 text-left font-mono text-xs"
      >
        <span className="text-console-text min-w-0 flex-1 truncate">
          {file.path}
        </span>
        <span className="text-console-muted shrink-0">
          {file.binary ? "binary" : `${String(changed)} changed`}
        </span>
      </button>
      {expanded && (
        <div className="space-y-2 px-2 pb-2">
          {file.binary ? (
            <p className="text-console-muted text-xs">
              git could not show the contents of a binary file.
            </p>
          ) : (
            file.hunks.map((hunk, index) => (
              <div
                key={`${String(index)}:${hunk.header}`}
                className="space-y-1"
              >
                <p className="text-console-muted truncate font-mono text-xs">
                  {hunk.header}
                </p>
                <DiffView lines={hunk.lines} />
              </div>
            ))
          )}
        </div>
      )}
    </section>
  );
}
