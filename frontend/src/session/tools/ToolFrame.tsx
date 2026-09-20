// The chrome every tool message shares: what ran, how it ended, and a body the
// registry chooses (`SPEC.md`, "Transcript rendering").

import { createElement, useState } from "react";
import type { ReactNode } from "react";

import { Spinner } from "../../components/Spinner";
import type { ToolMessage } from "../sessionStore";
import { toolRendererFor } from "./registry";

export interface ToolFrameProps {
  message: ToolMessage;
  /** The nested subagent transcript, when this tool ran one. */
  children?: ReactNode;
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

export function ToolFrame({ message, children }: ToolFrameProps) {
  // A subagent's meaning is its nested transcript, not the `Task` call's JSON,
  // so the body starts folded for one and open for every other tool.
  const [open, setOpen] = useState(message.subagent === undefined);

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
          className="text-console-text flex items-center gap-2 font-mono text-xs"
        >
          <span aria-hidden="true" className="text-console-muted">
            {open ? "▾" : "▸"}
          </span>
          <span>{message.name}</span>
        </button>
        <StatusMark message={message} />
        {message.truncated === true && (
          <span className="border-console-border text-console-muted rounded border px-1 text-[0.65rem]">
            truncated
          </span>
        )}
      </div>
      {open && (
        <div className="px-3 pt-1 pb-2">
          {/* The renderer is looked up per message, so it is created here
              rather than closed over by a component defined during render. */}
          {createElement(toolRendererFor(message.name), { message })}
        </div>
      )}
      {children}
    </div>
  );
}
