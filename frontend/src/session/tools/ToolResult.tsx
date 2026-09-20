// The result half of a tool body, shared by every renderer: text through the
// 40-line collapse, anything else through the JSON tree, tinted when the tool
// failed (`SPEC.md`, "Transcript rendering").

import { CollapsibleLines } from "../../components/CollapsibleLines";
import { JsonTree } from "../../components/JsonTree";
import type { ToolMessage } from "../sessionStore";
import { resultText } from "./toolInput";

export interface ToolResultProps {
  message: ToolMessage;
  /** Applied to the text before it is shown, e.g. `stripAnsi`. */
  transform?: (text: string) => string;
  /** Wrap long lines; off where columns carry meaning. */
  wrap?: boolean;
}

export function ToolResult({ message, transform, wrap }: ToolResultProps) {
  const raw = resultText(message.result);
  if (raw === null) {
    return message.result === undefined || message.result === null ? null : (
      <JsonTree value={message.result} name="result" />
    );
  }
  const text = transform ? transform(raw) : raw;
  if (text === "") {
    return null;
  }
  return (
    <CollapsibleLines
      text={text}
      label="result"
      wrap={wrap}
      className={message.is_error === true ? "text-state-failed" : ""}
    />
  );
}
