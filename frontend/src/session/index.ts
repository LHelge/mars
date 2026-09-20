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
