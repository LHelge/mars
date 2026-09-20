// Mirrors `SPEC.md`, "WebSocket: session stream": the JSON text frames of
// `GET /ws/sessions/{id}`, both directions, as unions discriminated on `type`
// so `useSessionSocket` can `switch` exhaustively.
//
// Terminal data travels as binary frames and is deliberately not part of these
// unions; only the `terminal_closed` notice is a JSON message.

import type { AgentEvent } from "./agentEvent";
import type { Session, SessionInput } from "./sessions";

export type ServerMessage =
  | { type: "event"; event: AgentEvent }
  /** Sent once right after the upgrade, then on every state change. */
  | { type: "session"; session: Session }
  | { type: "input_accepted"; client_id: string; seq: number }
  | { type: "input_rejected"; client_id: string; reason: string }
  /** `exit_code: -1` when the terminal could not start or ended without a code. */
  | { type: "terminal_closed"; exit_code: number }
  /** Followed by a close. */
  | { type: "error"; message: string };

export type ServerMessageType = ServerMessage["type"];

export type ClientMessage =
  /** `client_id` is client-generated and echoed back for optimistic UI. */
  | { type: "input"; client_id: string; input: SessionInput }
  | { type: "stop" }
  /** The session must be `running`. */
  | { type: "terminal_open"; cols: number; rows: number }
  | { type: "terminal_resize"; cols: number; rows: number }
  | { type: "terminal_close" };

export type ClientMessageType = ClientMessage["type"];
