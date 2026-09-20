// The body of a tool no family claims: MCP tools, tools a newer CLI added, and
// the `unknown` a result without a call becomes. Input and result are shown as
// a JSON tree (`SPEC.md`, "Transcript rendering").

import { JsonTree } from "../../components/JsonTree";
import type { ToolMessage } from "../sessionStore";
import { ToolResult } from "./ToolResult";

export interface JsonToolRendererProps {
  message: ToolMessage;
}

export function JsonToolRenderer({ message }: JsonToolRendererProps) {
  return (
    <div className="space-y-2">
      {message.input !== undefined && message.input !== null && (
        <JsonTree value={message.input} name="input" />
      )}
      <ToolResult message={message} />
    </div>
  );
}
