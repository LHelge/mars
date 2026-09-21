// Splitting a streaming markdown message into top-level blocks
// (`SPEC.md`, "Frontend", "Markdown").
//
// While an assistant message streams, every `text_delta` grows one string. If
// that string is re-parsed whole on every delta the cost of a message grows
// with its own length, exactly while the reader is watching it. The remedy is
// to freeze the part that can no longer change and re-parse only the tail.
//
// The split points are blank lines outside a code fence, and only where the
// text after the blank line cannot alter how the text before it parses. Two
// asymmetric risks decide every doubtful case:
//
//   * A wrong split is a rendering bug — a loose list torn into two lists, a
//     fence rendered as paragraphs.
//   * A late split is only slower.
//
// So the rules below are deliberately timid: anything indented, anything that
// continues a list or a blockquote, and anything inside a fence keeps the
// text in the growing tail.

/** The state a fence line puts the scanner in. */
interface Fence {
  char: "`" | "~";
  len: number;
}

// A fence opener, allowing the blockquote markers and the list indentation it
// may sit behind: a fence inside a quote or a list item still swallows blank
// lines, and treating a line as a fence when it is not only delays a split.
const FENCE = /^[ \t>]*(`{3,}|~{3,})(.*)$/;

// A list item marker at the very start of a line: `-`, `*`, `+`, `1.`, `1)`.
const LIST_ITEM = /^(?:[-*+]|\d{1,9}[.)])(?:[ \t]|$)/;

function fenceOf(line: string): { fence: Fence; info: string } | null {
  const match = FENCE.exec(line);
  if (!match) return null;
  const marker = match[1];
  return {
    fence: { char: marker[0] as "`" | "~", len: marker.length },
    info: match[2],
  };
}

/** Whether `line` closes `open`: same character, at least as long, no info. */
function closes(open: Fence, line: string): boolean {
  const found = fenceOf(line);
  if (!found) return false;
  return (
    found.fence.char === open.char &&
    found.fence.len >= open.len &&
    found.info.trim() === ""
  );
}

function isBlank(line: string): boolean {
  return line.trim() === "";
}

/** What a frozen block would be, as far as a following blank line cares. */
type BlockKind = "list" | "quote" | "other";

function kindOf(line: string): BlockKind {
  if (line.startsWith(">")) return "quote";
  if (LIST_ITEM.test(line)) return "list";
  return "other";
}

/**
 * Whether the block starting at `first` may be frozen when the next non-blank
 * line of the message is `next`. Everything that could still be pulled into
 * the same construct by what follows answers `false`.
 */
function mayFreeze(first: string, next: string): boolean {
  // Indented: a lazy continuation, an indented code block, or the body of a
  // list item — never a new top-level block.
  if (/^[ \t]/.test(next)) return false;
  const kind = kindOf(first);
  // A loose list is one list: items separated by blank lines.
  if (kind === "list" && LIST_ITEM.test(next)) return false;
  // Two quoted chunks separated by a blank line are still one blockquote as
  // far as the reader is concerned; keep them together.
  if (kind === "quote" && next.startsWith(">")) return false;
  return true;
}

/**
 * Splits `text` into blocks whose concatenation is exactly `text`.
 *
 * Every element but the last is complete: no later text can change how it
 * parses, so it may be rendered once and memoised. The last element is the
 * growing tail and is the only one a delta re-parses.
 *
 * A message with no blank line — one very long paragraph, one open fence —
 * is one block, which is today's behaviour and is the acceptable worst case.
 */
export function splitTopLevelBlocks(text: string): string[] {
  if (text === "") return [""];
  const lines = text.split("\n");
  const blocks: string[] = [];
  // Index into `lines` where the current block starts, and the first
  // non-blank line of that block, which decides what a blank line means.
  let start = 0;
  let first: string | null = null;
  let open: Fence | null = null;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (open) {
      if (closes(open, line)) open = null;
      continue;
    }
    const fence = fenceOf(line);
    if (fence) {
      first ??= line;
      open = fence.fence;
      continue;
    }
    if (!isBlank(line)) {
      first ??= line;
      continue;
    }
    // A blank line outside a fence. It is a split point only once further
    // text has arrived — a block is frozen after a blank line *followed by*
    // text, never at the end of what has been received so far.
    let next = i + 1;
    while (next < lines.length && isBlank(lines[next])) next += 1;
    if (next >= lines.length) break;
    // Leading blank lines: there is nothing yet to freeze, and they stay with
    // the block that follows them.
    if (first === null) continue;
    if (!mayFreeze(first, lines[next])) continue;
    // The blank run belongs to the block that ends, so that joining the
    // blocks reproduces the message byte for byte.
    blocks.push(lines.slice(start, next).join("\n") + "\n");
    start = next;
    first = null;
    i = next - 1;
  }

  blocks.push(lines.slice(start).join("\n"));
  return blocks;
}

// A link reference definition or a footnote definition: `[label]: …` at the
// start of a line. Both are resolved against the whole document, so a message
// carrying one is rendered whole rather than block by block.
const DEFINITION = /^ {0,3}\[[^\]\n]+\]:/m;

/**
 * Whether `text` carries a definition that only the whole document resolves —
 * a link reference or a footnote. Split rendering would leave such references
 * unresolved, so a completed message containing one is rendered in one piece.
 */
export function hasReferenceDefinitions(text: string): boolean {
  return DEFINITION.test(text);
}
