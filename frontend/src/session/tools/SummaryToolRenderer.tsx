// Reads, searches and fetches: one line saying what was asked for, with the
// full input and the result under it. The frame around it starts folded, so by
// the time this is drawn the user has asked to see the answer and it starts
// open; the line still folds it away (`SPEC.md`, "Transcript rendering").

import { JsonTree } from "../../components/JsonTree";
import { Disclosure } from "../Disclosure";
import type { ToolMessage } from "../sessionStore";
import { ToolResult } from "./ToolResult";
import { summaryLine } from "./toolInput";

export interface SummaryToolRendererProps {
  message: ToolMessage;
}

export function SummaryToolRenderer({ message }: SummaryToolRendererProps) {
  const summary = summaryLine(message.name, message.input);

  return (
    <div className="space-y-2">
      <Disclosure
        rowId={message.id}
        slot="summary"
        defaultOpen
        summaryClassName={`flex w-full items-center gap-2 text-left font-mono text-xs ${
          message.is_error === true ? "text-state-failed" : "text-console-text"
        }`}
        bodyClassName="space-y-2"
        summary={<span className="truncate">{summary}</span>}
      >
        {message.input !== undefined && message.input !== null && (
          <JsonTree value={message.input} name="input" />
        )}
        <ToolResult message={message} wrap={false} />
      </Disclosure>
    </div>
  );
}
