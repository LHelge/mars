// Assistant prose, rendered as markdown (`SPEC.md`, "Transcript rendering").
//
// The rendering itself is `MarkdownBody`, shared with the task description and
// task comments so every piece of written text in the application is set the
// same way. The measure cap lives on the text blocks inside it, not on this
// container, so a table or a code fence may use the whole row.
//
// It is reached through `StreamingMarkdown`, which parses only the growing
// tail while the message streams and coalesces deltas to one render per
// frame. A completed message goes through the same wrapper, so nothing is
// rebuilt when streaming ends.

import { StreamingMarkdown } from "../../components/StreamingMarkdown";
import { STREAMING_CURSOR } from "../../utils/testIds";
import type { AssistantTextMessage } from "../sessionStore";

export interface AssistantTextProps {
  message: AssistantTextMessage;
}

export function AssistantText({ message }: AssistantTextProps) {
  return (
    <div className="text-console-text">
      <StreamingMarkdown streaming={message.streaming}>
        {message.text}
      </StreamingMarkdown>
      {message.streaming && (
        <span
          data-testid={STREAMING_CURSOR}
          aria-hidden="true"
          className="bg-console-accent ml-0.5 inline-block h-[1em] w-[0.4em] animate-pulse align-text-bottom"
        />
      )}
    </div>
  );
}
