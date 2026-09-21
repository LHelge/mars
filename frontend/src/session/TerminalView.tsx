// The `Terminal` side panel: a PTY into the running session container
// (`SPEC.md`, "WebSocket: session stream", the `terminal_open` / binary frames
// / `terminal_close` / `terminal_closed` exchange, and "User-facing features",
// Sessions).
//
// It is an inspection escape hatch, nothing more: the bytes typed here go
// straight to an `exec` of `/bin/bash -l` as the `agent` user inside the
// container — the permission boundary of `ARCHITECTURE.md`, "Trust boundaries"
// — and nothing that happens in it is recorded as a transcript event. So the
// panel keeps no history, folds nothing into the session store and holds the
// `Terminal` in a ref rather than in state; React only owns whether an
// attachment exists.
//
// One attachment lives as long as one socket: the orchestrator disposes the PTY
// when the connection drops, so a reconnection leaves the panel detached with a
// `Reconnect` button rather than reattaching a dead terminal behind the
// operator's back. Every path out — the panel closing, the session leaving
// `running`, the socket dropping, unmount — sends `terminal_close` and
// disposes, and every path back in — a `Reconnect`, or a session that parks
// and runs again — starts from a cleared exit bar.
//
// The xterm imports, including its stylesheet, are deliberately confined to
// this module so it can be lazy-loaded: `sidePanels.ts` is the only importer.

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import "@xterm/xterm/css/xterm.css";

import { SubmitButton } from "../components";
import { debounce } from "../utils/debounce";
import { useSessionSocketApi } from "./SessionSocketContext";
import type { SessionPanelProps } from "./sidePanels";
import { CONSOLE_MONO, terminalTheme } from "./terminalTheme";

/** Long enough to sit out a drag of the panel edge, short enough to feel live. */
const RESIZE_DEBOUNCE_MS = 100;
const FONT_SIZE = 12;
const SCROLLBACK = 5000;

/** `onBinary` hands over one character per byte; send them back as bytes. */
function latin1(data: string): Uint8Array {
  const bytes = new Uint8Array(data.length);
  for (let index = 0; index < data.length; index += 1) {
    bytes[index] = data.charCodeAt(index) & 0xff;
  }
  return bytes;
}

function prefersLight(): boolean {
  return window.matchMedia("(prefers-color-scheme: light)").matches;
}

export function TerminalView({ session }: SessionPanelProps) {
  const socket = useSessionSocketApi();
  const terminal = socket.terminal;
  const live = socket.status === "live";
  const running = session.state === "running";

  const hostRef = useRef<HTMLDivElement | null>(null);
  /** The attachment's generation; `null` once it has been given up. */
  const [attachId, setAttachId] = useState<number | null>(0);
  const [exitCode, setExitCode] = useState<number | null>(null);
  /** The socket had reached `live` under this attachment. */
  const wasLive = useRef(false);

  useEffect(() => {
    if (live) {
      wasLive.current = true;
      return;
    }
    if (!wasLive.current) return;
    wasLive.current = false;
    // The server disposed the PTY along with the connection. Say so and wait
    // to be asked for a new one.
    setAttachId(null);
  }, [live]);

  useEffect(() => {
    const host = hostRef.current;
    if (attachId === null || !running || !live || host === null) return;

    // A new attachment is a new process, so whatever the last one exited with
    // is no longer what this panel is reporting. Reconnect is not the only way
    // back here: a session that parks and runs again detaches and re-attaches
    // on its own, and the exit bar of the shell that died with the old
    // container must not outlive it.
    setExitCode(null);

    const term = new Terminal({
      convertEol: false,
      cursorBlink: true,
      fontFamily: CONSOLE_MONO,
      fontSize: FONT_SIZE,
      scrollback: SCROLLBACK,
      theme: terminalTheme(prefersLight()),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);

    let disposed = false;
    let sent = { cols: 0, rows: 0 };

    const refit = (): void => {
      try {
        fit.fit();
      } catch {
        // A panel with no layout yet (hidden, zero-sized) has nothing to fit.
      }
    };

    const encoder = new TextEncoder();
    const unsubscribe = terminal.subscribe((frame) => {
      if (frame instanceof Uint8Array) {
        // xterm buffers and parses on its own schedule; one call per frame.
        term.write(frame);
        return;
      }
      term.write(`\r\n[process exited with code ${String(frame.exit_code)}]\r\n`);
      setExitCode(frame.exit_code);
    });
    const onData = term.onData((data) => {
      terminal.write(encoder.encode(data));
    });
    const onBinary = term.onBinary((data) => {
      terminal.write(latin1(data));
    });

    const resize = debounce(() => {
      refit();
      if (term.cols === sent.cols && term.rows === sent.rows) return;
      sent = { cols: term.cols, rows: term.rows };
      terminal.resize(sent.cols, sent.rows);
    }, RESIZE_DEBOUNCE_MS);
    const observer = new ResizeObserver(() => {
      resize.run();
    });
    observer.observe(host);

    const scheme = window.matchMedia("(prefers-color-scheme: light)");
    const onScheme = (): void => {
      term.options.theme = terminalTheme(scheme.matches);
    };
    scheme.addEventListener("change", onScheme);

    // Cell size depends on the metrics of the loaded font, so the fit whose
    // `cols`/`rows` the PTY is opened with waits for it.
    refit();
    void document.fonts.ready.then(() => {
      if (disposed) return;
      refit();
      sent = { cols: term.cols, rows: term.rows };
      terminal.open(sent.cols, sent.rows);
      term.focus();
    });

    return () => {
      disposed = true;
      scheme.removeEventListener("change", onScheme);
      observer.disconnect();
      resize.cancel();
      onData.dispose();
      onBinary.dispose();
      unsubscribe();
      terminal.close();
      term.dispose();
    };
  }, [attachId, live, running, terminal]);

  // Asking for a new attachment is all this does; the exit bar is cleared by
  // the attachment itself, wherever it came from.
  const reconnect = (): void => {
    setAttachId((id) => (id ?? 0) + 1);
  };

  if (!running) {
    return <Notice>Terminal is available while the session is running</Notice>;
  }

  if (attachId === null) {
    return (
      <Notice action={<Reconnect onClick={reconnect} />}>
        Terminal disconnected
      </Notice>
    );
  }

  if (!live) {
    return <Notice>Waiting for the session stream</Notice>;
  }

  return (
    <div className="bg-console-bg flex h-full min-h-0 flex-col">
      <div ref={hostRef} className="min-h-0 flex-1 overflow-hidden px-2 py-1" />
      {exitCode !== null && (
        <div className="border-console-border bg-console-surface flex items-center justify-between gap-2 border-t px-2 py-1.5">
          <span className="text-console-muted font-mono text-xs">
            {exitCode < 0
              ? "No terminal could be opened"
              : `Process exited with code ${String(exitCode)}`}
          </span>
          <Reconnect onClick={reconnect} />
        </div>
      )}
    </div>
  );
}

interface ReconnectProps {
  onClick: () => void;
}

function Reconnect({ onClick }: ReconnectProps) {
  return (
    <SubmitButton type="button" variant="ghost" onClick={onClick}>
      Reconnect
    </SubmitButton>
  );
}

interface NoticeProps {
  children: string;
  action?: ReactNode;
}

function Notice({ children, action }: NoticeProps) {
  return (
    <div className="flex h-full flex-col items-start gap-3 p-3">
      <p className="text-console-muted text-sm">{children}</p>
      {action}
    </div>
  );
}
