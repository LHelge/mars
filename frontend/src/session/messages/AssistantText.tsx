// Assistant prose, rendered as markdown (`SPEC.md`, "Transcript rendering").
//
// No `rehype-raw`: `react-markdown`'s default never executes HTML, and agent
// output is untrusted text that happens to be displayed unredacted (ADR 0027).

import Markdown from "react-markdown";
import type { Components } from "react-markdown";

import type { AssistantTextMessage } from "../sessionStore";

const COMPONENTS: Components = {
  a: ({ children, href }) => (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      className="text-console-accent underline underline-offset-2"
    >
      {children}
    </a>
  ),
  code: ({ children, className }) => (
    <code className={`font-mono text-[0.85em] ${className ?? ""}`}>
      {children}
    </code>
  ),
  pre: ({ children }) => (
    <pre className="border-console-border bg-console-bg my-2 overflow-x-auto rounded border px-3 py-2 font-mono text-xs">
      {children}
    </pre>
  ),
  ul: ({ children }) => (
    <ul className="my-1 list-disc space-y-0.5 pl-5">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="my-1 list-decimal space-y-0.5 pl-5">{children}</ol>
  ),
  p: ({ children }) => <p className="my-1">{children}</p>,
};

export interface AssistantTextProps {
  message: AssistantTextMessage;
}

export function AssistantText({ message }: AssistantTextProps) {
  return (
    <div className="text-console-text max-w-prose">
      <Markdown components={COMPONENTS}>{message.text}</Markdown>
      {message.streaming && (
        <span
          data-testid="streaming-cursor"
          aria-hidden="true"
          className="bg-console-accent ml-0.5 inline-block h-[1em] w-[0.4em] animate-pulse align-text-bottom"
        />
      )}
    </div>
  );
}
