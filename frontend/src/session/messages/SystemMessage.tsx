// Everything the orchestrator itself says: init, state changes, git outcomes,
// permission denials and errors. One line, with the payload behind a fold.

import { formatValue } from "../json";
import type { SystemMessage as SystemMessageData } from "../sessionStore";

const LEVEL_CLASS: Record<SystemMessageData["level"], string> = {
  info: "text-console-muted",
  warn: "text-state-parked",
  error: "text-state-failed",
};

export interface SystemMessageProps {
  message: SystemMessageData;
}

export function SystemMessage({ message }: SystemMessageProps) {
  const detail = formatValue(message.detail);
  return (
    <div className={`text-sm ${LEVEL_CLASS[message.level]}`}>
      <p className="font-mono text-xs">{message.text}</p>
      {detail !== "" && (
        <details>
          <summary className="text-console-muted cursor-pointer text-xs select-none">
            Detail
          </summary>
          <pre className="border-console-border bg-console-bg mt-1 overflow-x-auto rounded border px-3 py-2 font-mono text-xs">
            {detail}
          </pre>
        </details>
      )}
    </div>
  );
}
