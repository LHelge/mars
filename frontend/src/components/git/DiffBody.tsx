// A whole `Diff`, drawn (`SPEC.md`, "Frontend", "Changes panel"): the file
// list with its counts and truncation notice, then the patch itself, one
// collapsible block per file, with a click in the list scrolling to the block.
//
// The session's Changes panel and the task drawer's hand-off diff show the
// same thing about two different targets — a live session branch and the
// immutable commit a hand-off retained — so the drawing lives here and each
// caller only decides what to fetch and what to say above it.

import { useMemo, useRef, useState } from "react";

import type { Diff } from "../../types";
import type { PatchFile } from "../../utils/diff";
import { parseUnifiedPatch } from "../../utils/diff";
import { DiffView } from "../DiffView";
import { ChangesFileList } from "./ChangesFileList";

/** Above this many changed lines a file opens collapsed. */
const COLLAPSE_CHANGED_LINES = 500;

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

  const blocks = useRef(new Map<string, HTMLElement>());
  const scrollToFile = (path: string) => {
    blocks.current.get(path)?.scrollIntoView({ block: "start" });
  };

  return (
    <>
      <ChangesFileList
        diff={diff}
        binaryPaths={binaryPaths}
        onSelect={scrollToFile}
      />
      {files.map((file) => (
        <PatchFileBlock
          key={file.path}
          file={file}
          anchor={(element) => {
            if (element === null) {
              blocks.current.delete(file.path);
            } else {
              blocks.current.set(file.path, element);
            }
          }}
        />
      ))}
    </>
  );
}

interface PatchFileBlockProps {
  file: PatchFile;
  anchor: (element: HTMLElement | null) => void;
}

/** One file of the patch: a header that collapses it, and its hunks. */
function PatchFileBlock({ file, anchor }: PatchFileBlockProps) {
  const changed = file.hunks.reduce(
    (sum, hunk) =>
      sum + hunk.lines.filter((line) => line.type !== "context").length,
    0,
  );
  const [expanded, setExpanded] = useState(changed <= COLLAPSE_CHANGED_LINES);

  return (
    <section ref={anchor} className="border-console-border rounded border">
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
