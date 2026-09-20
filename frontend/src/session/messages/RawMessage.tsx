// Native output the translator did not recognise. Nothing is dropped, so it is
// shown whole, as the JSON tree of `SPEC.md`, "Transcript rendering" — folded
// behind a `raw` disclosure, because an unrecognised line is rarely the thing
// the reader came for.

import { JsonTree } from "../../components/JsonTree";
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
      <div className="mt-1">
        <JsonTree value={message.native} />
      </div>
    </details>
  );
}
