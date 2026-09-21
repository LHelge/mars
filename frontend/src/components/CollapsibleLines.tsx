// Long output is collapsed above 40 lines (`SPEC.md`, "Transcript
// rendering"). The head stays visible, so a row never becomes a blank box and
// the count in the control says what is still hidden.
//
// The expansion is the caller's to own when it has somewhere better to keep it:
// inside the transcript a row is unmounted as soon as the reader scrolls away
// from it, so `ToolResult` holds this state per message in `session/sessionUi`
// and passes it in. With nothing passed in it stays here, which is what a
// listing outside a virtualised list wants.

import { useState } from "react";

import { splitLines } from "../utils/lines";

// A module that renders a component may export nothing else, so the limit is
// a constant here rather than a shared export.
/** The line count above which output is collapsed. */
const COLLAPSE_LINES = 40;

export interface CollapsibleLinesProps {
  text: string;
  /** Named in the control's accessible label, so several blocks stay apart. */
  label?: string;
  /**
   * Wrap long lines. Off for shell output and other content whose columns
   * carry meaning: those scroll sideways instead.
   */
  wrap?: boolean;
  /** Extra classes for the `<pre>`, used for error tinting. */
  className?: string;
  /** Whether the whole output is shown, when the caller owns that state. */
  expanded?: boolean;
  /** Flips it; required with `expanded` and unused without it. */
  onToggle?: () => void;
}

export function CollapsibleLines({
  text,
  label = "output",
  wrap = true,
  className = "",
  expanded: controlled,
  onToggle,
}: CollapsibleLinesProps) {
  const [own, setOwn] = useState(false);
  const expanded = controlled ?? own;
  const toggle =
    onToggle ??
    (() => {
      setOwn((value) => !value);
    });
  // `splitLines`, so the count in the control is the count an editor shows:
  // the newline that ends the last line is a terminator, not a line of its
  // own, and 40 lines ending in one must not read "Show all 41 lines".
  const lines = splitLines(text);
  const total = lines.length;
  const long = total > COLLAPSE_LINES;
  const collapsed = long && !expanded;

  return (
    <div>
      <pre
        className={`border-console-border bg-console-bg overflow-x-auto rounded border px-3 py-2 font-mono text-xs ${
          wrap ? "whitespace-pre-wrap" : "whitespace-pre"
        } ${className}`}
      >
        {(collapsed ? lines.slice(0, COLLAPSE_LINES) : lines).join("\n")}
      </pre>
      {long && (
        <button
          type="button"
          onClick={toggle}
          aria-label={
            collapsed ? `Show all ${total} lines of ${label}` : `Collapse ${label}`
          }
          className="text-console-accent pt-1 text-xs underline underline-offset-2"
        >
          {collapsed ? `Show all ${total} lines` : "Collapse"}
        </button>
      )}
    </div>
  );
}
