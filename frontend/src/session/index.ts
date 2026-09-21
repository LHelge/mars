// The composer (`SPEC.md`, "Frontend", "Composer").
export { Composer } from "./Composer";
export type { ComposerProps } from "./Composer";

// The session view and its parts (`SPEC.md`, "Frontend", Routes: `/sessions/:id`).
export { SessionView } from "./SessionView";
export type { SessionViewProps } from "./SessionView";
export { SessionHeader } from "./SessionHeader";
export type { SessionHeaderProps } from "./SessionHeader";
// `SessionActions` itself is not re-exported: the store's action interface
// already owns that name here, and only the header ever renders the buttons.
// The file list is shared with the task drawer's hand-off diff, so it lives in
// the UI kit now; re-exported here because the panel it was built for is here.
export { ChangesFileList } from "../components/git/ChangesFileList";
export type { ChangesFileListProps } from "../components/git/ChangesFileList";
export { ChangesPanel } from "./ChangesPanel";
export { SidePanel } from "./SidePanel";
export type { SidePanelProps } from "./SidePanel";
export { TasksPanel } from "./TasksPanel";
// `TerminalView` is deliberately not re-exported: `sidePanels.ts` is its only
// importer, so the xterm bundle stays out of everything that touches this
// barrel and a later task can lazy-load it.
export { panelsFor, sidePanels } from "./sidePanels";
export type { SessionPanelProps, SidePanelEntry } from "./sidePanels";
export {
  SessionSocketContext,
  useSessionSocketApi,
} from "./SessionSocketContext";

// The session transcript store (`SPEC.md`, "Frontend", "Session state").

export {
  clearSessionStores,
  createSessionStore,
  disposeSessionStore,
  emptySessionState,
  foldEvent,
  getSessionStore,
  isNewerSession,
  MAX_RETAINED_SESSIONS,
  onSessionStoreCleared,
  optimisticId,
  peekSessionStore,
  releaseSessionStore,
  retainedSessionIds,
  retainSessionStore,
  sessionStoreGeneration,
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
export {
  byName,
  clearToolRenderers,
  registerToolRenderer,
  toolRendererFor,
} from "./tools/registry";
export type { ToolRenderer } from "./tools/registry";

// The tool families and the helpers they narrow tool payloads with
// (`SPEC.md`, "Transcript rendering").
export { registerBuiltinToolRenderers } from "./tools/renderers";
export { EditToolRenderer } from "./tools/EditToolRenderer";
export { JsonToolRenderer } from "./tools/JsonToolRenderer";
export { ShellToolRenderer } from "./tools/ShellToolRenderer";
export { SummaryToolRenderer } from "./tools/SummaryToolRenderer";
export { ToolResult } from "./tools/ToolResult";
export { resultText, summaryLine } from "./tools/toolInput";
