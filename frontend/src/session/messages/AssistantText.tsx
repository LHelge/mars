// Assistant prose, rendered as markdown (`SPEC.md`, "Transcript rendering").
//
// The rendering itself is `MarkdownBody`, shared with the task description and
// task comments so every piece of written text in the application is set the
// same way.

import { MarkdownBody } from "../../components/Markdown";
import type { AssistantTextMessage } from "../sessionStore";

export interface AssistantTextProps {
  message: AssistantTextMessage;
}

export function AssistantText({ message }: AssistantTextProps) {
  return (
    <div className="text-console-text max-w-prose">
      <MarkdownBody>{message.text}</MarkdownBody>
      {message.streaming && (
        <span
          data-testid="streaming-cursor"
          aria-hidden="true"
          className="bg-console-accent ml-0.5 inline-block h-[1em] w-[0.4em] animate-pulse align-text-bottom"
        />
      )}
    </div>
  );
}
