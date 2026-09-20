// The `Copy link` action of `SPEC.md`, "Frontend", "Copy links": the canonical
// route for the resource on the current origin, and nothing else — no search
// parameters, no fragment, no token.
//
// The clipboard is a permission, not a certainty: `Link copied` appears only
// after the write resolves, and a refusal (or a browser without the API, which
// includes every insecure origin) falls back to the URL in a selectable field
// so it can still be copied by hand.

import { useEffect, useRef, useState } from "react";

/** How long the confirmation stays before the button goes back to its label. */
const COPIED_MS = 2000;

export interface CopyLinkButtonProps {
  /** An application path, already absolute within the origin: `/sessions/{id}`. */
  path: string;
  /** What the fallback field is called, for the people who hear it read out. */
  label?: string;
}

export function CopyLinkButton({ path, label = "Link" }: CopyLinkButtonProps) {
  const [copied, setCopied] = useState(false);
  const [manual, setManual] = useState(false);
  const [shown, setShown] = useState(path);
  const fieldRef = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const url = `${window.location.origin}${path}`;

  useEffect(() => {
    return () => {
      if (timer.current !== null) clearTimeout(timer.current);
    };
  }, []);

  // A new resource is a new link: drop whatever the last one said, the React
  // way of reacting to a changed prop without an effect.
  if (path !== shown) {
    setShown(path);
    setCopied(false);
    setManual(false);
  }

  const copy = async (): Promise<void> => {
    const clipboard = navigator.clipboard as Clipboard | undefined;
    try {
      if (clipboard === undefined) throw new Error("no clipboard");
      await clipboard.writeText(url);
    } catch {
      setCopied(false);
      setManual(true);
      return;
    }
    setManual(false);
    setCopied(true);
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      setCopied(false);
    }, COPIED_MS);
  };

  // Selecting the text is the whole point of the fallback, so do it for them.
  useEffect(() => {
    if (manual) fieldRef.current?.select();
  }, [manual]);

  return (
    <span className="inline-flex min-w-0 items-center gap-2">
      <button
        type="button"
        onClick={() => {
          void copy();
        }}
        className="text-console-muted hover:text-console-text border-console-border hover:bg-console-raised rounded border px-2 py-0.5 font-mono text-xs"
      >
        {copied ? "Link copied" : "Copy link"}
      </button>
      {manual && (
        <input
          ref={fieldRef}
          readOnly
          aria-label={label}
          value={url}
          onFocus={(event) => {
            event.target.select();
          }}
          className="bg-console-surface border-console-border text-console-muted w-64 max-w-full rounded border px-2 py-0.5 font-mono text-xs"
        />
      )}
    </span>
  );
}
