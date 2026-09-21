// Reasoning, folded away by default. `redacted` is the backend's doing, not
// this view's (`SPEC.md`, "AgentEvent"), so the header says who redacted it.
//
// The text is the agent's own prose, so it is markdown like every other piece
// of written text (`SPEC.md`, "Transcript rendering"). The muted tone and the
// italics are set once on the container and inherited by the whole body,
// headings and code included: nothing inside `MarkdownBody` sets a text colour
// of its own except the elements that are already muted.

import { MarkdownBody } from "../../components/Markdown";
import type { ThinkingMessage as ThinkingMessageData } from "../sessionStore";

export interface ThinkingMessageProps {
  message: ThinkingMessageData;
}

export function ThinkingMessage({ message }: ThinkingMessageProps) {
  return (
    <details className="text-console-muted">
      <summary className="cursor-pointer text-xs select-none">
        {message.redacted ? "Thinking (redacted by backend)" : "Thinking"}
      </summary>
      <div className="border-console-border mt-1 border-l pl-3 text-sm italic">
        {/* Redacted thinking arrives with no text at all; the header above is
            the whole of what there is to show. */}
        {message.text !== "" && <MarkdownBody>{message.text}</MarkdownBody>}
      </div>
    </details>
  );
}
