// Opens the retained commit of one hand-off, wherever it is listed: beside the
// current hand-off, and on every row of the history.
//
// The panel decides what "open" means — clicking the row already on screen
// closes it again — so the button only names its hand-off.

import type { Handoff } from "../types";

export interface ViewDiffButtonProps {
  handoff: Handoff;
  onViewDiff: (handoff: Handoff) => void;
}

export function ViewDiffButton({ handoff, onViewDiff }: ViewDiffButtonProps) {
  return (
    <button
      type="button"
      onClick={() => {
        onViewDiff(handoff);
      }}
      className="border-console-border hover:bg-console-raised hover:text-console-text rounded border px-1.5 py-px font-mono text-[0.6875rem]"
    >
      View diff
    </button>
  );
}
