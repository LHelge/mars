// Opens the retained commit of one hand-off, wherever it is listed: beside the
// current hand-off, and on every row of the history.
//
// The panel decides what "open" means — clicking the row already on screen
// closes it again — so the button only names its hand-off.

import type { Handoff } from "../types";
import { CHIP_BUTTON } from "./taskChrome";

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
      className={CHIP_BUTTON}
    >
      View diff
    </button>
  );
}
