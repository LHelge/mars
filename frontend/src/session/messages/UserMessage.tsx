// What the operator said, and whether it got through.
//
// Deliberately literal: the text is shown `whitespace-pre-wrap` and never as
// markdown (`SPEC.md`, "Transcript rendering"), so a pasted diff keeps its
// leading `+`, an identifier keeps its underscores and a fence stays a fence.
// What a person typed is shown as typed.
//
// How far it got is the one `delivery` field of the message (`SPEC.md`,
// "Session state"): still on its way, or not sent — in which case the text is
// still here and one click puts it back in the composer.

import type { UserMessage as UserMessageData } from "../sessionStore";
import { isInFlight } from "../sessionStore";
import { requestResend } from "../sessionUi";
import { TAP_INLINE } from "../../components/fieldStyles";

export interface UserMessageProps {
  sessionId: string;
  message: UserMessageData;
}

export function UserMessage({ sessionId, message }: UserMessageProps) {
  const { delivery } = message;
  const inFlight = isInFlight(delivery);

  return (
    <div className="flex justify-end">
      <div
        className={`border-console-border bg-console-raised max-w-prose rounded border px-3 py-2 ${
          delivery.state === "rejected" ? "border-state-failed" : ""
        } ${inFlight ? "opacity-60" : ""}`}
      >
        <p className="text-console-text whitespace-pre-wrap">{message.text}</p>
        {inFlight && (
          <p className="text-console-muted pt-1 text-xs">sending…</p>
        )}
        {delivery.state === "rejected" && (
          <div className="flex items-center justify-end gap-3 pt-1">
            <p className="text-state-failed text-xs">
              Not sent: {delivery.reason}
            </p>
            <button
              type="button"
              onClick={() => {
                requestResend(sessionId, message.text);
              }}
              className={`text-console-accent text-xs underline underline-offset-2 ${TAP_INLINE}`}
            >
              Resend
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
