// The chrome every tool message shares: what ran, how it ended, and a body the
// registry chooses (`SPEC.md`, "Transcript rendering").

import { createElement } from "react";
import type { ReactNode } from "react";

import { Spinner } from "../../components/Spinner";
import { Disclosure } from "../Disclosure";
import type { ToolMessage } from "../sessionStore";
import { toolRendererFor } from "./registry";
import { SubagentToolRenderer } from "./SubagentToolRenderer";
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
  // rendering"). What the reader opens is remembered outside the row, so a row
  // the virtualizer recycles comes back as they left it (`sessionUi`).
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
      <Disclosure
        rowId={message.id}
        slot="tool"
        defaultOpen={defaultOpen}
        headerClassName="flex items-center gap-3 px-3 py-1.5"
        summaryClassName="text-console-text flex min-w-0 items-center gap-2 font-mono text-xs"
        bodyClassName="space-y-2 px-3 pt-1 pb-2"
        summary={(open) => (
          <>
            <span>{message.name}</span>
            {!open && summary !== "" && (
              <span className="text-console-muted truncate">{summary}</span>
            )}
          </>
        )}
        aside={
          <>
            <StatusMark message={message} />
            {message.truncated === true && (
              <span className="border-console-border text-console-muted rounded border px-1 text-[0.65rem]">
                truncated
              </span>
            )}
          </>
        }
      >
        {message.truncated === true && (
          <p className="text-console-muted text-xs">{TRUNCATED}</p>
        )}
        {/* The renderer is looked up per message, so it is created here
            rather than closed over by a component defined during render.
            A subagent's nested transcript is the group `MessageRow` already
            draws as `children`, so the body shows the call itself — the prompt
            that started the subagent, and the report it handed back — instead
            of repeating it. Before the `subagent_start` that says this call is
            one, no family claims `Task` or `Agent` and the JSON tree shows the
            input and the result like any other tool. */}
        {createElement(
          message.subagent === undefined
            ? toolRendererFor(message.name)
            : SubagentToolRenderer,
          { message },
        )}
      </Disclosure>
      {children}
    </div>
  );
}
