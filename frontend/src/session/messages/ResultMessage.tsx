// The end of a turn: what it cost and how long it took.

import type { ResultMessage as ResultMessageData } from "../sessionStore";

/**
 * A turn costs cents, so the two decimals `formatUsd` gives a column of
 * accumulated totals would round most turns to `$0.00`. The transcript shows
 * four.
 */
function formatTurnCost(value: number): string {
  return `$${value.toFixed(4)}`;
}

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
        <span>{formatTurnCost(message.cost_usd)}</span>
      )}
    </div>
  );
}
