// A fenced code block of `MarkdownBody` (`SPEC.md`, "Frontend", "Markdown"):
// a quiet header carrying the fence's language and a `Copy` action, over the
// code in the bordered monospace box the rest of the console uses.
//
// The block paints as plain text and stays plain until the highlighter chunk
// has been fetched and has run (`./highlight`, reached only through a dynamic
// import). Nothing is suspended and nothing is swapped for a spinner: the same
// `pre > code`, the same font and the same line height are on the page the
// whole time, so the upgrade from text to coloured spans moves nothing.
//
// Two things keep the highlighter off the hot path. It is attempted only after
// the code has stopped changing for `SETTLE_MS`, so a fence that is still
// growing delta by delta while a message streams is highlighted once, at the
// end, rather than on every keystroke of the agent. And while it grows, the
// spans already computed for the prefix are kept and the new tail is shown as
// plain text, so a streaming block never flickers back to plain.
//
// A block that is merely enormous — a pasted log — is left plain for good:
// colouring it would cost more than it is worth and would be read as a frozen
// tab.

import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import { Icon, ICON_CLASS } from "./icons";
import { useClipboardCopy } from "./useClipboardCopy";

/** How long the code must hold still before the highlighter is asked. */
const SETTLE_MS = 120;

/** Above either limit the block stays plain text. */
const MAX_CHARS = 200 * 1024;
const MAX_LINES = 2000;

interface Highlighted {
  /** The exact code these nodes were computed from. */
  code: string;
  nodes: ReactNode[];
}

function oversize(code: string): boolean {
  if (code.length > MAX_CHARS) return true;
  let lines = 1;
  for (let i = 0; i < code.length; i += 1) {
    if (code.charCodeAt(i) === 10) {
      lines += 1;
      if (lines > MAX_LINES) return true;
    }
  }
  return false;
}

export interface CodeBlockProps {
  /** The fence's contents, exactly as written: this is what `Copy` writes. */
  code: string;
  /** The fence's info string, when it had one. */
  language?: string;
}

export function CodeBlock({ code, language }: CodeBlockProps) {
  const [done, setDone] = useState<Highlighted | null>(null);
  const codeRef = useRef<HTMLElement>(null);
  const { copied, denied, copy } = useClipboardCopy();

  const plain = language === undefined || oversize(code);

  useEffect(() => {
    if (plain || language === undefined) return;
    if (done !== null && done.code === code) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      void import("./highlight").then(
        (module) => {
          if (cancelled) return;
          const nodes = module.highlightCode(language, code);
          if (nodes !== null) setDone({ code, nodes });
        },
        () => {
          // The chunk did not load; the block stays readable as plain text.
        },
      );
    }, SETTLE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [code, language, plain, done]);

  // Highlighted spans for as much of the code as has been highlighted, and the
  // rest as text. When the code is not an extension of what was highlighted —
  // an edit rather than a stream — the whole block is text until the next run.
  let body: ReactNode = code;
  if (done !== null && code.startsWith(done.code)) {
    body =
      done.code === code ? (
        done.nodes
      ) : (
        <>
          {done.nodes}
          {code.slice(done.code.length)}
        </>
      );
  }

  // The clipboard may refuse; then the code itself is selected, which is this
  // block's version of the field `CopyLinkButton` reveals.
  useEffect(() => {
    if (!denied) return;
    const element = codeRef.current;
    if (element === null) return;
    const selection = window.getSelection();
    if (selection === null || typeof document.createRange !== "function") {
      return;
    }
    const range = document.createRange();
    range.selectNodeContents(element);
    selection.removeAllRanges();
    selection.addRange(range);
  }, [denied]);

  return (
    <div className="border-console-border bg-console-bg my-2 overflow-hidden rounded border">
      <div className="border-console-border bg-console-surface text-console-muted flex items-center justify-between gap-2 border-b px-3 py-1">
        <span className="truncate font-mono text-[0.7rem] tracking-wide lowercase">
          {language ?? ""}
        </span>
        <button
          type="button"
          onClick={() => {
            void copy(code);
          }}
          className="text-console-muted hover:text-console-text hover:bg-console-raised inline-flex shrink-0 items-center gap-1.5 rounded px-1.5 py-0.5 font-mono text-[0.7rem]"
        >
          {copied ? (
            <Icon.copied aria-hidden="true" className={ICON_CLASS} />
          ) : (
            <Icon.copy aria-hidden="true" className={ICON_CLASS} />
          )}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre className="overflow-x-auto px-3 py-2 font-mono text-xs [overflow-wrap:normal]">
        <code ref={codeRef}>{body}</code>
      </pre>
      {denied && (
        <p className="text-console-muted border-console-border border-t px-3 py-1 font-mono text-[0.7rem]">
          Clipboard unavailable — the code is selected, copy it by hand.
        </p>
      )}
    </div>
  );
}
