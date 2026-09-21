// The body of a subagent's tool row: the call that started it, and the report
// it handed back (`SPEC.md`, "Transcript rendering").
//
// The nested transcript is drawn by `MessageRow` as the frame's children, so
// this body shows the call itself — as a JSON tree, like any input no family
// claims — and then the subagent's final report. That report is prose an agent
// wrote, so it is markdown, not the monospace an ordinary tool's program output
// gets. A result that is not text at all keeps the JSON tree.
//
// The 40-line collapse of `ToolResult` does not apply: a line count over
// rendered markdown would cut a table or a fence in half. A tall report scrolls
// inside its own box instead. Both are only reached once the row is opened, so
// a report of tens of kilobytes is parsed when the reader asks for it.

import { MarkdownBody } from "../../components/Markdown";
import { JsonTree } from "../../components/JsonTree";
import type { ToolMessage } from "../sessionStore";
import { resultText } from "./toolInput";
import { ToolResult } from "./ToolResult";

export interface SubagentToolRendererProps {
  message: ToolMessage;
}

export function SubagentToolRenderer({ message }: SubagentToolRendererProps) {
  const report = resultText(message.result);

  return (
    <div className="space-y-2">
      {message.input !== undefined && message.input !== null && (
        <JsonTree value={message.input} name="input" />
      )}
      {report === null ? (
        // Not text: the JSON tree, through the shared result body.
        <ToolResult message={message} />
      ) : (
        report.trim() !== "" && (
          <div
            className={`max-h-96 overflow-y-auto ${
              message.is_error === true ? "text-state-failed" : ""
            }`}
          >
            <MarkdownBody>{report}</MarkdownBody>
          </div>
        )
      )}
    </div>
  );
}
