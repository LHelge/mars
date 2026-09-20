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
