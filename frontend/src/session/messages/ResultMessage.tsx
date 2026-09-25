// The end of a turn: how long it took and what the conversation has cost so
// far. The backend reports a running total, not the turn's own cost, so it is
// labelled as one; the session header carries the session's sum.

import type { ResultMessage as ResultMessageData } from "../sessionStore";
import { COST_DECIMALS, formatUsd } from "../../utils/format";

function formatDuration(ms: number): string {
  return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
}

export interface ResultMessageProps {
  message: ResultMessageData;
}

export function ResultMessage({ message }: ResultMessageProps) {
  return (
    <div
      className={`border-console-border flex flex-wrap items-center gap-x-4 gap-y-1 rounded border border-dashed px-3 py-1.5 font-mono text-xs ${
        message.is_error
          ? "border-state-failed text-state-failed"
          : "text-console-muted"
      }`}
    >
      <span>{message.subtype}</span>
      <span>
        {message.num_turns} {message.num_turns === 1 ? "turn" : "turns"}
      </span>
      <span>{formatDuration(message.duration_ms)}</span>
      {message.cost_usd !== undefined && (
        <span title="The agent's running total at the end of this turn, not this turn's own cost">
          total {formatUsd(message.cost_usd, COST_DECIMALS)}
        </span>
      )}
    </div>
  );
}
