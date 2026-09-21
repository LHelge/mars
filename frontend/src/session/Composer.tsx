// The session composer of `SPEC.md`, "Frontend", "Composer": a text area that
// sends a `message` input on Enter, relabels its button "Interject" while a
// turn is in progress, offers a stop button, is absent for ephemeral sessions
// and is disabled once the session is `done` or `failed`.
//
// There is no answer mode: the pinned CLI never asks the host a question, so
// `SessionInput` has the single kind `message` (ADR 0033). A message sent
// mid-turn is queued by the CLI as the next turn.

import { useCallback, useEffect, useRef, useState } from "react";

import { Alert } from "../components/Alert";
import { SubmitButton } from "../components/SubmitButton";
import { useSessionSocketApi } from "./SessionSocketContext";
import { getSessionStore, optimisticId, useSessionStore } from "./sessionStore";
import { useResendRequest } from "./sessionUi";
import { useStopSession } from "./useStopSession";

/** The text area grows to this many lines and then scrolls. */
const MAX_LINES = 8;
/** Past this length a message is worth a word of warning, never a refusal. */
const LARGE_TEXT = 100 * 1024;
/**
 * Safari fires the Enter that confirms an IME composition *after*
 * `compositionend`, with `isComposing` already false and this legacy key code
 * in its place. Submitting on it turns the act of accepting a candidate into
 * sending the message.
 */
const IME_KEY_CODE = 229;

export interface ComposerProps {
  sessionId: string;
}

function number(value: string): number {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

/** Grows the text area with its content, up to `MAX_LINES`, then scrolls. */
function autoGrow(area: HTMLTextAreaElement): void {
  area.style.height = "auto";
  const style = getComputedStyle(area);
  const lineHeight = number(style.lineHeight) || 20;
  const extra =
    number(style.paddingTop) +
    number(style.paddingBottom) +
    number(style.borderTopWidth) +
    number(style.borderBottomWidth);
  const max = lineHeight * MAX_LINES + extra;
  const wanted = area.scrollHeight;
  area.style.height = `${String(Math.min(wanted, max))}px`;
  area.style.overflowY = wanted > max ? "auto" : "hidden";
}

export function Composer({ sessionId }: ComposerProps) {
  const socket = useSessionSocketApi();
  const session = useSessionStore(sessionId, (state) => state.session);
  const turnActive = useSessionStore(sessionId, (state) => state.turnActive);
  const lastRejection = useSessionStore(
    sessionId,
    (state) => state.lastRejection,
  );

  const resend = useResendRequest(sessionId);

  const [text, setText] = useState("");
  const [consumedResend, setConsumedResend] = useState(0);
  // The same stop the header's button makes, including what it says while the
  // request is outstanding.
  const stop = useStopSession(sessionId, socket.stop);
  const areaRef = useRef<HTMLTextAreaElement>(null);

  const state = session?.state;
  const ended = state === "done" || state === "failed";
  const running = state === "running";

  // The transcript hands a rejected message back to be edited and resent. Its
  // button is a plain handler that records the request; the composer adopts it
  // here, once per token, which is the React way of reacting to changed state
  // without an effect and a render of its own.
  if (resend !== null && resend.token !== consumedResend) {
    setConsumedResend(resend.token);
    setText(resend.text);
  }

  useEffect(() => {
    const area = areaRef.current;
    if (area !== null) autoGrow(area);
  }, [text]);

  useEffect(() => {
    // The one thing adopting the text cannot do during render: put the cursor
    // where the user is about to edit.
    if (consumedResend === 0) return;
    areaRef.current?.focus();
  }, [consumedResend]);

  // The store is the external system the composer reacts to: a rejection puts
  // the refused text back in the box. (A session that leaves `running` ends a
  // stop in progress; that half lives in `useStopSession`.)
  useEffect(() => {
    return getSessionStore(sessionId).subscribe((next, previous) => {
      const rejection = next.lastRejection;
      if (rejection === null || rejection === previous.lastRejection) return;
      const message = next.messages[optimisticId(rejection.client_id)];
      if (message?.kind !== "user") return;
      // Only into an empty box: whatever the user has typed since outranks
      // the message the orchestrator refused.
      setText((current) => (current.trim() === "" ? message.text : current));
    });
  }, [sessionId]);

  const send = useCallback(() => {
    const trimmed = text.trim();
    if (trimmed === "" || ended) return;
    socket.send({ kind: "message", text: trimmed });
    setText("");
    areaRef.current?.focus();
  }, [ended, socket, text]);

  // Ephemeral sessions run one prompt and end: they take no further input
  // (`SPEC.md`, "User-facing features", Sessions), so there is no composer.
  // Until the `session` frame arrives the kind is unknown, so nothing renders.
  if (!session || session.kind === "ephemeral") return null;

  // A socket that gave up sends over HTTP exactly as a reconnecting one does
  // (`send`, below): the difference is in the banner above the transcript,
  // not here.
  const disconnected =
    socket.status === "reconnecting" || socket.status === "offline";

  const hint =
    state === "parked"
      ? "Sending will relaunch the session"
      : state === "creating"
        ? "Queued until the session starts"
        : state === "done"
          ? "Session has ended"
          : state === "failed"
            ? "Session failed — use Retry to relaunch"
            : null;

  return (
    <form
      className="border-console-border bg-console-bg border-t p-3"
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      {lastRejection !== null && (
        <div className="mb-2">
          <Alert kind="error">Not sent: {lastRejection.reason}</Alert>
        </div>
      )}
      {text.length > LARGE_TEXT && (
        <div className="mb-2">
          <Alert kind="warning">
            That is a lot of text. It will be sent as one message.
          </Alert>
        </div>
      )}

      <textarea
        ref={areaRef}
        aria-label="Message"
        rows={2}
        disabled={ended}
        value={text}
        placeholder={ended ? "" : "Message the session"}
        onChange={(event) => {
          setText(event.target.value);
        }}
        onKeyDown={(event) => {
          if (event.key !== "Enter") return;
          // IME composition ends on an Enter that must not submit.
          if (event.nativeEvent.isComposing) return;
          if (event.nativeEvent.keyCode === IME_KEY_CODE) return;
          if (event.shiftKey && !(event.metaKey || event.ctrlKey)) return;
          event.preventDefault();
          send();
        }}
        className="bg-console-surface border-console-border text-console-text placeholder:text-console-muted block w-full resize-none rounded border px-3 py-2 font-mono text-sm disabled:opacity-60"
      />

      <div className="mt-2 flex items-center gap-3">
        <p className="text-console-muted min-w-0 flex-1 text-xs">
          {hint}
          {hint !== null && disconnected && " — "}
          {disconnected && "Offline, sending over HTTP"}
        </p>
        {running && (
          <SubmitButton
            type="button"
            variant="danger"
            disabled={stop.stopping}
            onClick={stop.requestStop}
          >
            {stop.label}
          </SubmitButton>
        )}
        <SubmitButton disabled={ended || text.trim() === ""}>
          {turnActive && running ? "Interject" : "Send"}
        </SubmitButton>
      </div>
    </form>
  );
}
