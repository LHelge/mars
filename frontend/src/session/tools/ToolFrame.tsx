// The chrome every tool message shares: what ran, how it ended, and a body the
// registry chooses (`SPEC.md`, "Transcript rendering").

import { createElement, useState } from "react";
import type { ReactNode } from "react";

import { Spinner } from "../../components/Spinner";
import type { ToolMessage } from "../sessionStore";
import { JsonToolRenderer } from "./JsonToolRenderer";
import { toolRendererFor } from "./registry";
// Populates the registry: a tool row is never drawn without this module.
import "./renderers";
import { headerSummary } from "./toolInput";

/** What a result the orchestrator had to cut says about itself. */
const TRUNCATED =
  "Result truncated at 256 KiB; the full output is in the session transcript file";

export interface ToolFrameProps {
  message: ToolMessage;
  /** The nested subagent transcript, when this tool ran one. */
  children?: ReactNode;
  /** Start with the body showing; the transcript never does. */
  defaultOpen?: boolean;
}

function StatusMark({ message }: { message: ToolMessage }) {
  if (message.running) {
    return (
      <span className="text-state-running flex items-center gap-1 text-xs">
        <Spinner className="size-3" />
        <span>running</span>
      </span>
    );
  }
  return message.is_error ? (
    <span className="text-state-failed text-xs">failed</span>
  ) : (
    <span className="text-console-muted text-xs">done</span>
  );
}

export function ToolFrame({
  message,
  children,
  defaultOpen = false,
}: ToolFrameProps) {
  // Every body starts folded: a working session is mostly tool calls, and the
  // header's one line says what each of them was (`SPEC.md`, "Transcript
  // rendering").
  const [open, setOpen] = useState(defaultOpen);
  const summary =
    message.subagent === undefined
      ? headerSummary(message.name, message.input)
      : "";

  return (
    <div
      className={`rounded border ${
        message.is_error === true
          ? "border-state-failed bg-state-failed/5"
          : "border-console-border bg-console-surface"
      }`}
    >
      <div className="flex items-center gap-3 px-3 py-1.5">
        <button
          type="button"
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
          className="text-console-text flex min-w-0 items-center gap-2 font-mono text-xs"
        >
          <span aria-hidden="true" className="text-console-muted">
            {open ? "▾" : "▸"}
          </span>
          <span>{message.name}</span>
          {!open && summary !== "" && (
            <span className="text-console-muted truncate">{summary}</span>
          )}
        </button>
        <StatusMark message={message} />
        {message.truncated === true && (
          <span className="border-console-border text-console-muted rounded border px-1 text-[0.65rem]">
            truncated
          </span>
        )}
      </div>
      {open && (
        <div className="space-y-2 px-3 pt-1 pb-2">
          {message.truncated === true && (
            <p className="text-console-muted text-xs">{TRUNCATED}</p>
          )}
          {/* The renderer is looked up per message, so it is created here
              rather than closed over by a component defined during render.
              A subagent's registered renderer is the nested group `MessageRow`
              already draws as `children`, so the body shows the call itself —
              the prompt that started the subagent — instead of repeating it. */}
          {createElement(
            message.subagent === undefined
              ? toolRendererFor(message.name)
              : JsonToolRenderer,
            { message },
          )}
        </div>
      )}
      {children}
    </div>
  );
}
