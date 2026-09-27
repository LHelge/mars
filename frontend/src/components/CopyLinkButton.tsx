// The `Copy link` action of `SPEC.md`, "Frontend", "Copy links": the canonical
// route for the resource on the current origin, and nothing else — no search
// parameters, no fragment, no token.
//
// The clipboard is a permission, not a certainty: `Link copied` appears only
// after the write resolves, and a refusal (or a browser without the API, which
// includes every insecure origin) falls back to the URL in a selectable field
// so it can still be copied by hand. That rule lives in `useCopyToClipboard`,
// which the hand-off panel's commit copy follows too, over the same
// `useClipboardCopy` that `CodeBlock`'s `Copy` uses.

import { Icon, ICON_CLASS } from "./icons";
import { useCopyToClipboard } from "./useClipboardCopy";
import { TAP_INLINE, TOUCH_TEXT } from "./fieldStyles";

export interface CopyLinkButtonProps {
  /** An application path, already absolute within the origin: `/sessions/{id}`. */
  path: string;
  /** What the fallback field is called, for the people who hear it read out. */
  label?: string;
}

export function CopyLinkButton({ path, label = "Link" }: CopyLinkButtonProps) {
  const url = `${window.location.origin}${path}`;
  // A new resource is a new link: the hook drops whatever the last one said.
  const { copied, manual, copy, fieldRef } = useCopyToClipboard(url);
  const Glyph = copied ? Icon.copied : Icon.link;

  return (
    <span className="inline-flex min-w-0 items-center gap-2">
      <button
        type="button"
        onClick={copy}
        className={`text-console-muted hover:text-console-text border-console-border hover:bg-console-raised inline-flex items-center gap-1.5 rounded border px-2 py-0.5 font-mono text-xs ${TAP_INLINE}`}
      >
        <Glyph aria-hidden="true" className={ICON_CLASS} />
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
          className={`bg-console-surface border-console-border text-console-muted w-64 max-w-full rounded border px-2 py-0.5 font-mono text-xs ${TOUCH_TEXT}`}
        />
      )}
    </span>
  );
}
