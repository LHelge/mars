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
import { getSessionStore, optimisticId, useSessionStore } from "./sessionStore";
import type { SessionSocketApi } from "./useSessionSocket";

/** The text area grows to this many lines and then scrolls. */
const MAX_LINES = 8;
/** Past this length a message is worth a word of warning, never a refusal. */
const LARGE_TEXT = 100 * 1024;
/**
 * A stop is a request, not a transaction (`ARCHITECTURE.md`, "Stop
 * semantics"): if no state change arrives, the button becomes usable again
 * rather than staying stuck.
 */
const STOP_TIMEOUT_MS = 30_000;

export interface ComposerProps {
  sessionId: string;
  socket: SessionSocketApi;
  /** Text to start from, e.g. the transcript's resend of a rejected message. */
  initialText?: string;
  /** Called once `initialText` has been taken into the text area. */
  onConsumedInitialText?: () => void;
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

export function Composer({
  sessionId,
  socket,
  initialText,
  onConsumedInitialText,
}: ComposerProps) {
  const session = useSessionStore(sessionId, (state) => state.session);
  const turnActive = useSessionStore(sessionId, (state) => state.turnActive);
  const lastRejection = useSessionStore(
    sessionId,
    (state) => state.lastRejection,
  );

  const [text, setText] = useState(initialText ?? "");
  const [consumedInitial, setConsumedInitial] = useState(initialText);
  const [stopping, setStopping] = useState(false);
  const areaRef = useRef<HTMLTextAreaElement>(null);
  const stopTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const state = session?.state;
  const ended = state === "done" || state === "failed";
  const running = state === "running";

  // The transcript hands a rejected message back to be edited and resent; a
  // new `initialText` is adopted as it arrives, the React way of reacting to a
  // changed prop without an effect.
  if (initialText !== consumedInitial) {
    setConsumedInitial(initialText);
    if (initialText !== undefined && initialText !== "") setText(initialText);
  }

  useEffect(() => {
    const area = areaRef.current;
    if (area !== null) autoGrow(area);
  }, [text]);

  useEffect(() => {
    if (consumedInitial === undefined || consumedInitial === "") return;
    areaRef.current?.focus();
    onConsumedInitialText?.();
  }, [consumedInitial, onConsumedInitialText]);

  // The store is the external system the composer reacts to: a rejection puts
  // the refused text back in the box, and a session that leaves `running`
  // ends a stop in progress.
  useEffect(() => {
    return getSessionStore(sessionId).subscribe((next, previous) => {
      if (next.session?.state !== "running") {
        setStopping(false);
      }
      const rejection = next.lastRejection;
      if (rejection === null || rejection === previous.lastRejection) return;
      const message = next.messages[optimisticId(rejection.client_id)];
      if (message?.kind !== "user") return;
      // Only into an empty box: whatever the user has typed since outranks
      // the message the orchestrator refused.
      setText((current) => (current.trim() === "" ? message.text : current));
    });
  }, [sessionId]);

  useEffect(() => {
    return () => {
      if (stopTimer.current !== null) clearTimeout(stopTimer.current);
    };
  }, []);

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
          if (event.shiftKey && !(event.metaKey || event.ctrlKey)) return;
          event.preventDefault();
          send();
        }}
        className="bg-console-surface border-console-border text-console-text placeholder:text-console-muted block w-full resize-none rounded border px-3 py-2 font-mono text-sm disabled:opacity-60"
      />

      <div className="mt-2 flex items-center gap-3">
        <p className="text-console-muted min-w-0 flex-1 text-xs">
          {hint}
          {hint !== null && socket.status === "reconnecting" && " — "}
          {socket.status === "reconnecting" && "Offline, sending over HTTP"}
        </p>
        {running && (
          <SubmitButton
            type="button"
            variant="danger"
            disabled={stopping}
            onClick={() => {
              setStopping(true);
              socket.stop();
              if (stopTimer.current !== null) clearTimeout(stopTimer.current);
              stopTimer.current = setTimeout(() => {
                setStopping(false);
              }, STOP_TIMEOUT_MS);
            }}
          >
            {stopping ? "Stopping…" : "Stop"}
          </SubmitButton>
        )}
        <SubmitButton disabled={ended || text.trim() === ""}>
          {turnActive && running ? "Interject" : "Send"}
        </SubmitButton>
      </div>
    </form>
  );
}
