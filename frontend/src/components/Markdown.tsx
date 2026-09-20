// Markdown prose, wherever the application shows text a human or an agent
// wrote: assistant messages (`SPEC.md`, "Transcript rendering"), task
// descriptions and task comments.
//
// No `rehype-raw`: `react-markdown`'s default never executes HTML, and both
// agent output and user text are untrusted text that happens to be displayed
// unredacted (ADR 0027).

import Markdown from "react-markdown";
import type { Components } from "react-markdown";

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

export interface MarkdownBodyProps {
  children: string;
}

export function MarkdownBody({ children }: MarkdownBodyProps) {
  return <Markdown components={COMPONENTS}>{children}</Markdown>;
}
