// Why a `failed` session failed, as text on its row rather than as the state
// pill's tooltip (`SPEC.md`, "Frontend", "Mobile layout": a `title` is never
// the only place a fact lives).
//
// The orchestrator's `error` can be long — a container's last line of stderr —
// so the row shows two lines of it and a `Show all` that lifts the clamp in
// place. Whether two lines are too few is not measured: a reason past
// `CLAMP_CHARS` offers the toggle, which on a wide screen may unclamp what
// already fitted, and costs nothing when it does.

import { useId, useState } from "react";

import { TAP_INLINE } from "./fieldStyles";

/** Past this, two lines of a narrow cell are likely not the whole reason. */
const CLAMP_CHARS = 90;

export interface SessionFailureReasonProps {
  error: string;
}

export function SessionFailureReason({ error }: SessionFailureReasonProps) {
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  const long = error.length > CLAMP_CHARS;

  return (
    <div className="mt-0.5 text-xs">
      <p
        id={id}
        className={`text-console-muted font-mono break-words ${expanded ? "" : "line-clamp-2"}`}
      >
        {error}
      </p>
      {long && (
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={id}
          onClick={() => {
            setExpanded(!expanded);
          }}
          className={`text-console-accent hover:underline ${TAP_INLINE}`}
        >
          {expanded ? "Show less" : "Show all"}
        </button>
      )}
    </div>
  );
}
