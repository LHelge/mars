// A subagent's own transcript, nested under the tool call that started it
// (`SPEC.md`, "Session state" and "Transcript rendering").
//
// The nested list is not virtualised: it is bounded by one subagent's output
// and it lives inside a row the outer virtualizer measures, which can only
// measure what is actually in the DOM. Depth is unbounded and drawn plain, so a
// subagent inside a subagent indents once more and nothing else changes.

import { useState } from "react";
import type { ReactNode } from "react";

import { SUBAGENT_CHILDREN } from "../utils/testIds";
import type { ToolMessage } from "./sessionStore";

export interface SubagentGroupProps {
  message: ToolMessage;
  /**
   * The nested rows, rendered only while expanded. Optional because the tool
   * registry resolves this component for `Task` and `Agent`, and a resolved
   * renderer is only handed the message.
   */
  children?: ReactNode;
}

export function SubagentGroup({ message, children }: SubagentGroupProps) {
  // Folded whether it is running or has ended: the frame above says `running`,
  // and the nested transcript is one click away.
  const [open, setOpen] = useState(false);
  const subagent = message.subagent;
  const failed = subagent?.is_error === true;

  return (
    <div className="border-console-border border-t">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs"
      >
        <span aria-hidden="true" className="text-console-muted">
          {open ? "▾" : "▸"}
        </span>
        <span className={failed ? "text-state-failed" : "text-state-human"}>
          {subagent?.agent_type ?? "Agent"}
        </span>
        <span className="text-console-muted truncate">
          {subagent?.description ?? ""}
        </span>
      </button>
      {open && (
        <div
          data-testid={SUBAGENT_CHILDREN}
          className="border-console-border ml-4 space-y-2 border-l pt-1 pb-2 pl-3"
        >
          {children}
        </div>
      )}
    </div>
  );
}
