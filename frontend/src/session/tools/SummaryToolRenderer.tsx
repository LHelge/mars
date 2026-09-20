// Reads, searches and fetches: one line saying what was asked for, because the
// answer is usually only interesting when something went wrong. Clicking opens
// the full input and the result (`SPEC.md`, "Transcript rendering").

import { useState } from "react";

import { JsonTree } from "../../components/JsonTree";
import type { ToolMessage } from "../sessionStore";
import { ToolResult } from "./ToolResult";
import { summaryLine } from "./toolInput";

export interface SummaryToolRendererProps {
  message: ToolMessage;
}

export function SummaryToolRenderer({ message }: SummaryToolRendererProps) {
  const [open, setOpen] = useState(false);
  const summary = summaryLine(message.name, message.input);

  return (
    <div className="space-y-2">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        className={`flex w-full items-center gap-2 text-left font-mono text-xs ${
          message.is_error === true ? "text-state-failed" : "text-console-text"
        }`}
      >
        <span aria-hidden="true" className="text-console-muted">
          {open ? "▾" : "▸"}
        </span>
        <span className="truncate">{summary}</span>
      </button>
      {open && (
        <div className="space-y-2">
          {message.input !== undefined && message.input !== null && (
            <JsonTree value={message.input} name="input" />
          )}
          <ToolResult message={message} wrap={false} />
        </div>
      )}
    </div>
  );
}
