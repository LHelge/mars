// The body of a tool nothing more specific claims: its input and its result,
// printed as JSON. The per-family renderers of `SPEC.md`, "Transcript
// rendering", register themselves over this one.

import { formatValue } from "../json";
import type { ToolMessage } from "../sessionStore";
import { CollapsibleText } from "./CollapsibleText";

export interface DefaultToolRendererProps {
  message: ToolMessage;
}

export function DefaultToolRenderer({ message }: DefaultToolRendererProps) {
  const input = formatValue(message.input);
  const result = formatValue(message.result);
  return (
    <div className="space-y-2">
      {input !== "" && <CollapsibleText text={input} label="input" />}
      {result !== "" && <CollapsibleText text={result} label="result" />}
    </div>
  );
}
