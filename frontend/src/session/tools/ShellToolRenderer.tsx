// Shell tools: the command that ran, then its output in monospace with the
// escape sequences stripped (`SPEC.md`, "Transcript rendering"). Output is not
// wrapped — a column of a `ls -l` or a stack trace means what its position
// says — so a long line scrolls sideways instead of reflowing.

import { stripAnsi } from "../../utils/ansi";
import type { ToolMessage } from "../sessionStore";
import { JsonToolRenderer } from "./JsonToolRenderer";
import { ToolResult } from "./ToolResult";
import { isShellInput } from "./toolInput";

export interface ShellToolRendererProps {
  message: ToolMessage;
}

export function ShellToolRenderer({ message }: ShellToolRendererProps) {
  if (!isShellInput(message.input)) {
    return <JsonToolRenderer message={message} />;
  }
  const { command, description } = message.input;

  return (
    <div className="space-y-2">
      {description !== undefined && description !== "" && (
        <p className="text-console-muted text-xs">{description}</p>
      )}
      <pre className="border-console-border bg-console-bg text-console-text overflow-x-auto rounded border px-3 py-2 font-mono text-xs whitespace-pre">
        <span aria-hidden="true" className="text-console-accent select-none">
          ${" "}
        </span>
        <span>{command}</span>
      </pre>
      <ToolResult message={message} transform={stripAnsi} wrap={false} />
    </div>
  );
}
