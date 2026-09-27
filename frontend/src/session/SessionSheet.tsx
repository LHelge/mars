// The session view's one sheet frame: a dialog over the lower part of the
// session box, above an overlay that dims what it covers.
//
// A phone has no room for a column beside the transcript or a band that grows
// the header, so what would be one there — the side panel below `lg`, the
// header's branch section below `sm` — opens as this sheet instead (`SPEC.md`,
// "Frontend", "Session side panel" and "Session header"). Escape, the caller's
// close button or a tap on the overlay dismisses it; where the focus goes on
// opening and closing is the caller's, since only it knows its opener.
//
// Both halves are `absolute`: the sheet covers the nearest positioned ancestor,
// which is what decides how much of the session box it lays over.

import type { ReactNode, Ref } from "react";

export interface SessionSheetProps {
  /** The dialog's accessible name. */
  label: string;
  onClose: () => void;
  children: ReactNode;
  ref?: Ref<HTMLDivElement>;
}

export function SessionSheet({
  label,
  onClose,
  children,
  ref,
}: SessionSheetProps) {
  return (
    <>
      {/* The overlay dims what is under the sheet and takes the tap that
          dismisses it. It is a sibling of the transcript's scroller, not a
          child, so neither the tap nor a drag on it reaches that scroller's
          stick-to-bottom handler. */}
      <div
        aria-hidden="true"
        onClick={onClose}
        className="absolute inset-0 z-10 touch-none bg-black/50"
      />
      <div
        ref={ref}
        role="dialog"
        aria-label={label}
        onKeyDown={(event) => {
          // A key the content handled itself — the terminal's own Escape, a
          // form's — is the content's, not the sheet's.
          if (event.key !== "Escape" || event.defaultPrevented) return;
          event.preventDefault();
          onClose();
        }}
        className="border-console-border bg-console-surface absolute inset-x-0 bottom-0 z-20 flex h-[85%] flex-col rounded-t-lg border-t shadow-2xl"
      >
        {children}
      </div>
    </>
  );
}
