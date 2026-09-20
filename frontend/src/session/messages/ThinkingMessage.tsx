// Reasoning, folded away by default. `redacted` is the backend's doing, not
// this view's (`SPEC.md`, "AgentEvent"), so the header says who redacted it.

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
      <p className="border-console-border mt-1 max-w-prose border-l pl-3 text-sm whitespace-pre-wrap italic">
        {message.text}
      </p>
    </details>
  );
}
