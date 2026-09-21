// One transcript row: the store's message kind decides the renderer.
//
// The row subscribes to its own `messages[id]` rather than receiving the
// message from the list, so a `text_delta` that grows one message re-renders
// one row and leaves the rest of the transcript alone.
//
// The subscription is only half of that: `Transcript` itself re-renders on
// every delta — it follows the tail — and would rebuild the element for every
// row in the viewport. Nothing in this build memoises for us (there is no
// React Compiler; `ARCHITECTURE.md`, "Frontend architecture"), so the row is
// `memo()`-wrapped by hand. Its props are stable across those re-renders: the
// id comes from `order`, `depth` is a number, and `onResend` is the `setResend`
// state setter of `SessionView`. A prop that is a fresh object or closure per
// render would silently undo this.

import { memo } from "react";

import { SubagentGroup } from "../SubagentGroup";
import type { Message } from "../sessionStore";
import { useSessionStore } from "../sessionStore";
import { ToolFrame } from "../tools/ToolFrame";
import { AssistantText } from "./AssistantText";
import { RawMessage } from "./RawMessage";
import { ResultMessage } from "./ResultMessage";
import { SystemMessage } from "./SystemMessage";
import { ThinkingMessage } from "./ThinkingMessage";
import { UserMessage } from "./UserMessage";

/**
 * The gutter glyph, quiet enough to scan past and specific enough to find.
 * Keyed by `Message["kind"]` and not by `string`, so a new message kind is a
 * compile error here rather than an empty gutter nobody notices.
 */
const GLYPH: Record<Message["kind"], string> = {
  user: "›",
  assistant_text: "",
  thinking: "~",
  tool: "",
  system: "·",
  result: "",
  raw: "",
};

export interface MessageRowProps {
  sessionId: string;
  id: string;
  /** Nesting depth; subagent rows are drawn plain, only indented. */
  depth?: number;
  onResend?: (text: string) => void;
}

export const MessageRow = memo(function MessageRow({
  sessionId,
  id,
  depth = 0,
  onResend,
}: MessageRowProps) {
  const message = useSessionStore(sessionId, (state) => state.messages[id]);
  // Children are resolved from `subagents`, never from `order`: a subagent
  // nested inside a subagent has no top-level position at all.
  const toolUseId = message?.kind === "tool" ? message.tool_use_id : null;
  const childIds = useSessionStore(sessionId, (state) =>
    toolUseId === null ? undefined : state.subagents[toolUseId],
  );

  if (!message) {
    return null;
  }

  let body;
  switch (message.kind) {
    case "user":
      body = <UserMessage message={message} onResend={onResend} />;
      break;
    case "assistant_text":
      body = <AssistantText message={message} />;
      break;
    case "thinking":
      body = <ThinkingMessage message={message} />;
      break;
    case "system":
      body = <SystemMessage message={message} />;
      break;
    case "result":
      body = <ResultMessage message={message} />;
      break;
    case "raw":
      body = <RawMessage message={message} />;
      break;
    case "tool":
      body = (
        <ToolFrame message={message}>
          {message.subagent && (
            <SubagentGroup message={message}>
              {(childIds ?? []).map((childId) => (
                <MessageRow
                  key={childId}
                  sessionId={sessionId}
                  id={childId}
                  depth={depth + 1}
                  onResend={onResend}
                />
              ))}
            </SubagentGroup>
          )}
        </ToolFrame>
      );
      break;
    default: {
      // Every `Message` kind is rendered above, or this stops compiling. The
      // arm itself is unreachable: the store's fold produces only these kinds,
      // and an `AgentEvent` kind it does not know becomes a `raw` message.
      const unhandled: never = message;
      body = <RawMessage message={unhandled} />;
      break;
    }
  }

  return (
    <div data-message-kind={message.kind} className="flex gap-2 py-1">
      <span
        aria-hidden="true"
        className="text-console-muted w-3 shrink-0 pt-0.5 text-right font-mono text-xs"
      >
        {GLYPH[message.kind]}
      </span>
      <div className="min-w-0 flex-1">{body}</div>
    </div>
  );
});
