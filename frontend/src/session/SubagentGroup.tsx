// A subagent's own transcript, nested under the tool call that started it
// (`SPEC.md`, "Session state" and "Transcript rendering").
//
// The nested list is not virtualised: it is bounded by one subagent's output
// and it lives inside a row the outer virtualizer measures, which can only
// measure what is actually in the DOM. Depth is unbounded and drawn plain, so a
// subagent inside a subagent indents once more and nothing else changes.

import type { ReactNode } from "react";

import { SUBAGENT_CHILDREN } from "../utils/testIds";
import { Disclosure } from "./Disclosure";
import type { ToolMessage } from "./sessionStore";

export interface SubagentGroupProps {
  message: ToolMessage;
  /** The nested rows, rendered only while expanded. */
  children?: ReactNode;
}

export function SubagentGroup({ message, children }: SubagentGroupProps) {
  // Folded whether it is running or has ended: the frame above says `running`,
  // and the nested transcript is one click away. A subagent the reader is
  // following stays open while they look elsewhere, because the fold's state
  // is held outside this row (`sessionUi`).
  const subagent = message.subagent;
  const failed = subagent?.is_error === true;

  return (
    <div className="border-console-border border-t">
      <Disclosure
        rowId={message.id}
        slot="subagent"
        summaryClassName="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs"
        bodyClassName="border-console-border ml-4 space-y-2 border-l pt-1 pb-2 pl-3"
        bodyTestId={SUBAGENT_CHILDREN}
        summary={
          <>
            <span className={failed ? "text-state-failed" : "text-state-human"}>
              {subagent?.agent_type ?? "Agent"}
            </span>
            <span className="text-console-muted truncate">
              {subagent?.description ?? ""}
            </span>
          </>
        }
      >
        {children}
      </Disclosure>
    </div>
  );
}
