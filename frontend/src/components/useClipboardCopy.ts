// The one clipboard rule this UI follows, in one place: the clipboard is a
// permission, not a certainty (`SPEC.md`, "Frontend", "Copy links").
//
// The confirmation appears only after the write resolves, and a refusal — or a
// browser without the API at all, which includes every insecure origin — is
// reported as `denied` so the caller can leave the text somewhere it can still
// be copied by hand: a selectable field beside a link, the code itself
// selected in a fence.

import { useCallback, useEffect, useRef, useState } from "react";

/** How long a confirmation stays before the caller goes back to its label. */
export const COPIED_MS = 2000;

export interface ClipboardCopy {
  /** The write resolved, and has not yet timed out. */
  copied: boolean;
  /** The last attempt was refused, or there was no clipboard API. */
  denied: boolean;
  /** Attempt the write; never rejects. */
  copy: (text: string) => Promise<void>;
  /** Forget both, for a caller whose subject changed under it. */
  reset: () => void;
}

export function useClipboardCopy(): ClipboardCopy {
  const [copied, setCopied] = useState(false);
  const [denied, setDenied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current !== null) clearTimeout(timer.current);
    };
  }, []);

  const reset = useCallback(() => {
    setCopied(false);
    setDenied(false);
  }, []);

  const copy = useCallback(async (text: string): Promise<void> => {
    const clipboard = navigator.clipboard as Clipboard | undefined;
    try {
      if (clipboard === undefined) throw new Error("no clipboard");
      await clipboard.writeText(text);
    } catch {
      setCopied(false);
      setDenied(true);
      return;
    }
    setDenied(false);
    setCopied(true);
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      setCopied(false);
    }, COPIED_MS);
  }, []);

  return { copied, denied, copy, reset };
}
