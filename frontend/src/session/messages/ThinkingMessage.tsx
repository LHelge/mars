// Reasoning, folded away by default. `redacted` is the backend's doing, not
// this view's (`SPEC.md`, "AgentEvent"), so the header says who redacted it.
//
// The text is the agent's own prose, so it is markdown like every other piece
// of written text (`SPEC.md`, "Transcript rendering"). The muted tone and the
// italics are set once on the container and inherited by the whole body,
// headings and code included: nothing inside `MarkdownBody` sets a text colour
// of its own except the elements that are already muted. The fold is the
// transcript's own `Disclosure`, so the body is parsed when the reader asks for
// it and what they opened survives the row being recycled.

import { MarkdownBody } from "../../components/Markdown";
import { Disclosure } from "../Disclosure";
import type { ThinkingMessage as ThinkingMessageData } from "../sessionStore";

export interface ThinkingMessageProps {
  message: ThinkingMessageData;
}

export function ThinkingMessage({ message }: ThinkingMessageProps) {
  return (
    <div className="text-console-muted">
      <Disclosure
        rowId={message.id}
        slot="thinking"
        summaryClassName="flex items-center gap-1 text-xs select-none"
        bodyClassName="border-console-border mt-1 border-l pl-3 text-sm italic"
        summary={
          <span>
            {message.redacted ? "Thinking (redacted by backend)" : "Thinking"}
          </span>
        }
      >
        {/* Redacted thinking arrives with no text at all; the header above is
            the whole of what there is to show. */}
        {message.text !== "" && <MarkdownBody>{message.text}</MarkdownBody>}
      </Disclosure>
    </div>
  );
}
