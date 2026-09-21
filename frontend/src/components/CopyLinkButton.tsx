// The `Copy link` action of `SPEC.md`, "Frontend", "Copy links": the canonical
// route for the resource on the current origin, and nothing else — no search
// parameters, no fragment, no token.
//
// The clipboard is a permission, not a certainty: `Link copied` appears only
// after the write resolves, and a refusal (or a browser without the API, which
// includes every insecure origin) falls back to the URL in a selectable field
// so it can still be copied by hand. That rule lives in `useClipboardCopy`,
// which `CodeBlock`'s `Copy` follows too.

import { useEffect, useRef, useState } from "react";

import { useClipboardCopy } from "./useClipboardCopy";

export interface CopyLinkButtonProps {
  /** An application path, already absolute within the origin: `/sessions/{id}`. */
  path: string;
  /** What the fallback field is called, for the people who hear it read out. */
  label?: string;
}

export function CopyLinkButton({ path, label = "Link" }: CopyLinkButtonProps) {
  const { copied, denied: manual, copy, reset } = useClipboardCopy();
  const [shown, setShown] = useState(path);
  const fieldRef = useRef<HTMLInputElement>(null);

  const url = `${window.location.origin}${path}`;

  // A new resource is a new link: drop whatever the last one said, the React
  // way of reacting to a changed prop without an effect.
  if (path !== shown) {
    setShown(path);
    reset();
  }

  // Selecting the text is the whole point of the fallback, so do it for them.
  useEffect(() => {
    if (manual) fieldRef.current?.select();
  }, [manual]);

  return (
    <span className="inline-flex min-w-0 items-center gap-2">
      <button
        type="button"
        onClick={() => {
          void copy(url);
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
