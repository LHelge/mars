// What the operator said, and whether it got through.
//
// Deliberately literal: the text is shown `whitespace-pre-wrap` and never as
// markdown (`SPEC.md`, "Transcript rendering"), so a pasted diff keeps its
// leading `+`, an identifier keeps its underscores and a fence stays a fence.
// What a person typed is shown as typed.

import type { UserMessage as UserMessageData } from "../sessionStore";

export interface UserMessageProps {
  message: UserMessageData;
  /** Called with the rejected text so the composer can send it again. */
  onResend?: (text: string) => void;
}

export function UserMessage({ message, onResend }: UserMessageProps) {
  const rejected = message.rejected !== undefined;
  return (
    <div className="flex justify-end">
      <div
        className={`border-console-border bg-console-raised max-w-prose rounded border px-3 py-2 ${
          rejected ? "border-state-failed" : ""
        } ${message.pending ? "opacity-60" : ""}`}
      >
        <p className="text-console-text whitespace-pre-wrap">{message.text}</p>
        {message.pending && (
          <p className="text-console-muted pt-1 text-xs">sending…</p>
        )}
        {rejected && (
          <div className="flex items-center justify-end gap-3 pt-1">
            <p className="text-state-failed text-xs">
              Not sent: {message.rejected}
            </p>
            {onResend && (
              <button
                type="button"
                onClick={() => onResend(message.text)}
                className="text-console-accent text-xs underline underline-offset-2"
              >
                Resend
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
