// The transcript's one fold: a ▾/▸ control over a body that is rendered only
// while it is open (`SPEC.md`, "Transcript rendering").
//
// Every collapsible thing in a row — the tool body, a subagent's nested
// transcript, a read's summary, thinking, a raw line, a system message's detail
// — is this component, so the glyph, the `aria-expanded` and the rule that a
// folded body costs nothing to hold are written once. The open state is not
// held here: it lives in `sessionUi`, keyed by the row's message id, because a
// virtualised row is unmounted as soon as the reader scrolls a few screens away
// and anything held inside it would be lost.

import type { ReactNode } from "react";

import { disclosureKey, useDisclosure } from "./sessionUi";

export interface DisclosureProps {
  /** The message this fold belongs to: what its state is keyed by. */
  rowId: string;
  /** Which fold of that message, where a row has more than one. */
  slot: string;
  /** How it starts until the reader says otherwise. */
  defaultOpen?: boolean;
  /**
   * The clickable line beside the glyph. A function is given the current state,
   * for a header that says something different while it is folded.
   */
  summary: ReactNode | ((open: boolean) => ReactNode);
  /** Beside the control and outside the button: status marks, badges. */
  aside?: ReactNode;
  /** Wraps the control and `aside`. */
  headerClassName?: string;
  /** The control itself. */
  summaryClassName?: string;
  /** The body, which exists only while open. */
  bodyClassName?: string;
  bodyTestId?: string;
  children: ReactNode;
}

export function Disclosure({
  rowId,
  slot,
  defaultOpen = false,
  summary,
  aside,
  headerClassName = "",
  summaryClassName = "",
  bodyClassName = "",
  bodyTestId,
  children,
}: DisclosureProps) {
  const [open, toggle] = useDisclosure(
    disclosureKey(rowId, slot),
    defaultOpen,
  );

  return (
    <>
      <div className={headerClassName}>
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          className={summaryClassName}
        >
          <span aria-hidden="true" className="text-console-muted">
            {open ? "▾" : "▸"}
          </span>
          {typeof summary === "function" ? summary(open) : summary}
        </button>
        {aside}
      </div>
      {open && (
        <div className={bodyClassName} data-testid={bodyTestId}>
          {children}
        </div>
      )}
    </>
  );
}
