// Native output the translator did not recognise. Nothing is dropped, so it is
// shown verbatim; the JSON tree of `SPEC.md`, "Transcript rendering", replaces
// this pretty-print in the tool-renderer task.

import { formatValue } from "../json";
import type { RawMessage as RawMessageData } from "../sessionStore";

export interface RawMessageProps {
  message: RawMessageData;
}

export function RawMessage({ message }: RawMessageProps) {
  return (
    <details>
      <summary className="text-console-muted cursor-pointer font-mono text-xs select-none">
        raw
      </summary>
      <pre className="border-console-border bg-console-bg mt-1 overflow-x-auto rounded border px-3 py-2 font-mono text-xs">
        {formatValue(message.native)}
      </pre>
    </details>
  );
}
