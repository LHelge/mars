// Native output the translator did not recognise. Nothing is dropped, so it is
// shown whole, as the JSON tree of `SPEC.md`, "Transcript rendering" — folded
// behind a `raw` disclosure, because an unrecognised line is rarely the thing
// the reader came for.

import { JsonTree } from "../../components/JsonTree";
import { Disclosure } from "../Disclosure";
import type { RawMessage as RawMessageData } from "../sessionStore";

export interface RawMessageProps {
  message: RawMessageData;
}

export function RawMessage({ message }: RawMessageProps) {
  return (
    <Disclosure
      rowId={message.id}
      slot="raw"
      summaryClassName="text-console-muted flex items-center gap-1 font-mono text-xs select-none"
      bodyClassName="mt-1"
      summary={<span>raw</span>}
    >
      <JsonTree value={message.native} />
    </Disclosure>
  );
}
