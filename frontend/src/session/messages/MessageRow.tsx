// One transcript row: the store's message kind decides the renderer.
//
// The row subscribes to its own `messages[id]` rather than receiving the
// message from the list, so a `text_delta` that grows one message re-renders
// one row and leaves the rest of the transcript alone.

import { SubagentGroup } from "../SubagentGroup";
import { useSessionStore } from "../sessionStore";
import { ToolFrame } from "../tools/ToolFrame";
import { AssistantText } from "./AssistantText";
import { RawMessage } from "./RawMessage";
import { ResultMessage } from "./ResultMessage";
import { SystemMessage } from "./SystemMessage";
import { ThinkingMessage } from "./ThinkingMessage";
import { UserMessage } from "./UserMessage";

/** The gutter glyph, quiet enough to scan past and specific enough to find. */
const GLYPH: Record<string, string> = {
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

export function MessageRow({
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
}
