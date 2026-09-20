// Long tool output is collapsed above 40 lines (`SPEC.md`, "Transcript
// rendering"). The head stays visible, so a row never becomes a blank box.

import { useState } from "react";

import { lineCount } from "../json";

/** The line count above which output is collapsed. */
const COLLAPSE_LINES = 40;

export interface CollapsibleTextProps {
  text: string;
  /** Read by tests and by the expand control's label. */
  label?: string;
}

export function CollapsibleText({ text, label = "output" }: CollapsibleTextProps) {
  const [expanded, setExpanded] = useState(false);
  const total = lineCount(text);
  const collapsed = !expanded && total > COLLAPSE_LINES;
  const shown = collapsed
    ? text.split("\n").slice(0, COLLAPSE_LINES).join("\n")
    : text;

  return (
    <div>
      <pre className="border-console-border bg-console-bg overflow-x-auto rounded border px-3 py-2 font-mono text-xs whitespace-pre-wrap">
        {shown}
      </pre>
      {total > COLLAPSE_LINES && (
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="text-console-accent pt-1 text-xs underline underline-offset-2"
        >
          {collapsed
            ? `Show all ${total} lines of ${label}`
            : `Collapse ${label}`}
        </button>
      )}
    </div>
  );
}
