// Markdown that is still being written (`SPEC.md`, "Frontend", "Markdown").
//
// `MarkdownBody` parses the string it is handed. A streaming assistant
// message hands it a longer string on every `text_delta`, so the whole
// message is re-parsed for every few characters that arrive and the cost of
// one delta grows with the message. This wrapper makes that cost proportional
// to the unfinished tail instead:
//
//   * The text is split at top-level block boundaries (`streamingBlocks.ts`).
//     Everything but the last block is complete and is rendered through a
//     memoised `MarkdownBody`, which React then leaves alone; only the last,
//     growing block is re-parsed when a delta arrives.
//   * Deltas are coalesced to at most one re-render per animation frame. Text
//     is never dropped: the frame renders whatever has arrived by the time it
//     runs, and the completed message always renders immediately.
//
// A completed message renders through this same path, so the transcript's DOM
// is not rebuilt when streaming ends — no scroll jump, and the nodes above
// the cursor keep their identity. The one exception is a message carrying a
// link reference or footnote definition: those resolve against the whole
// document, which block-by-block rendering cannot do, so such a message is
// rendered in one piece once it is complete.

import { memo, useEffect, useMemo, useRef, useState } from "react";

import { MarkdownBody } from "./Markdown";
import {
  hasReferenceDefinitions,
  splitTopLevelBlocks,
} from "./streamingBlocks";

// A heading drops its top margin at the start of a message (`first:mt-0` in
// `Markdown.tsx`). One message rendered as several bodies would drop it at the
// start of every block, so a heading in the middle of a message would sit
// tight against the paragraph above it; each body but the first puts the
// heading scale back. The variants are more specific than the `first:mt-0`
// they override, which is what makes them win.
const CONTINUES =
  "[&>h1:first-child]:mt-3 [&>h2:first-child]:mt-3 [&>h3:first-child]:mt-2 " +
  "[&>h4:first-child]:mt-2 [&>h5:first-child]:mt-2 [&>h6:first-child]:mt-2";

// The completed blocks: keyed by position and memoised on their content, so a
// delta that only grows the tail re-renders nothing above it. Keying on the
// content itself would instead throw away the growing block's DOM on every
// delta, which is the opposite of what this is for.
const Block = memo(function Block({
  text,
  continues,
}: {
  text: string;
  continues: boolean;
}) {
  return (
    <MarkdownBody className={continues ? CONTINUES : undefined}>
      {text}
    </MarkdownBody>
  );
});

/**
 * The text to render this frame. While `streaming`, a changed `text` is shown
 * on the next animation frame and every delta that arrives until then is
 * folded into it — the frame renders whatever has arrived when it runs, so no
 * text is ever dropped and the last delta always books a frame of its own. A
 * completed message is not deferred at all: it is rendered as given.
 */
function useFrameCoalesced(text: string, streaming: boolean): string {
  const [shown, setShown] = useState(text);
  const latest = useRef(text);
  const frame = useRef<number | null>(null);

  useEffect(() => {
    latest.current = text;
    if (!streaming || text === shown) {
      // Nothing more is coming, or nothing has changed: a booked frame would
      // only re-render what is already on screen.
      if (frame.current !== null) {
        cancelAnimationFrame(frame.current);
        frame.current = null;
      }
      return;
    }
    // A frame is already booked: it will pick up this delta and any that
    // follow it, so nothing is lost by not booking another.
    if (frame.current !== null) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      setShown(latest.current);
    });
  }, [text, shown, streaming]);

  useEffect(
    () => () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    },
    [],
  );

  // The completed text is rendered whole rather than as of the last frame:
  // the final `text` event must never be left waiting behind a frame.
  return streaming ? shown : text;
}

export interface StreamingMarkdownProps {
  /** The message text as folded so far. */
  children: string;
  /** Whether more deltas are still expected. */
  streaming: boolean;
}

export function StreamingMarkdown({
  children,
  streaming,
}: StreamingMarkdownProps) {
  const text = useFrameCoalesced(children, streaming);
  const whole = !streaming && hasReferenceDefinitions(text);
  const blocks = useMemo(
    () => (whole ? [text] : splitTopLevelBlocks(text)),
    [text, whole],
  );

  return (
    <>
      {blocks.map((block, index) => (
        // The index is the key: blocks are only ever appended, so a frozen
        // block never changes and never moves. Keying on the content instead
        // would replace the growing block's DOM on every delta.
        <Block key={index} text={block} continues={index > 0} />
      ))}
    </>
  );
}
