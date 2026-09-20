// The composer (`SPEC.md`, "Frontend", "Composer").
export { Composer } from "./Composer";
export type { ComposerProps } from "./Composer";

// The session transcript store (`SPEC.md`, "Frontend", "Session state").

export {
  createSessionStore,
  disposeSessionStore,
  emptySessionState,
  foldEvent,
  getSessionStore,
  optimisticId,
  useSessionStore,
} from "./sessionStore";
export type {
  AssistantTextMessage,
  ConnectionStatus,
  Message,
  RawMessage,
  ResultMessage,
  SessionActions,
  SessionState,
  SessionStore,
  SubagentInfo,
  SystemMessage,
  ThinkingMessage,
  ToolMessage,
  UserMessage,
} from "./sessionStore";

// The session WebSocket (`SPEC.md`, "WebSocket: session stream").
export { buildSessionSocketUrl } from "./socketUrl";
export { SessionSocket, useSessionSocket } from "./useSessionSocket";
export type {
  SessionSocketApi,
  SocketFactory,
  SocketLike,
  TerminalApi,
  TerminalFrame,
  TerminalListener,
} from "./useSessionSocket";

// The transcript view (`SPEC.md`, "Frontend", "Transcript rendering"). The
// per-kind renderers are not re-exported: their component names would collide
// with the message type names above, and only `MessageRow` ever picks one.

export { SubagentGroup } from "./SubagentGroup";
export type { SubagentGroupProps } from "./SubagentGroup";
export { Transcript } from "./Transcript";
export type { TranscriptProps } from "./Transcript";
export { useStickToBottom } from "./useStickToBottom";
export type { StickToBottom, StickToBottomOptions } from "./useStickToBottom";
export { MessageRow } from "./messages/MessageRow";
export type { MessageRowProps } from "./messages/MessageRow";
export { ToolFrame } from "./tools/ToolFrame";
export type { ToolFrameProps } from "./tools/ToolFrame";
export { DefaultToolRenderer } from "./tools/DefaultToolRenderer";
export {
  clearToolRenderers,
  registerToolRenderer,
  toolRendererFor,
} from "./tools/registry";
export type { ToolRenderer } from "./tools/registry";
