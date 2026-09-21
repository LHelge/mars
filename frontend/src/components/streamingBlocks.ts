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

/**
 * The line at `index`, or `""` past the end. A line that is not there is blank
 * for every purpose here — `isBlank`, `kindOf` and `mayFreeze` all read it the
 * same way — so the fallback is the behaviour the walk already had.
 */
function lineAt(lines: string[], index: number): string {
  return lines[index] ?? "";
}

function fenceOf(line: string): { fence: Fence; info: string } | null {
  const match = FENCE.exec(line);
  if (!match) return null;
  const marker = match[1];
  const info = match[2];
  // Both groups are part of every match `FENCE` produces; the check is what
  // says so, and it costs the same as the `!match` above.
  if (marker === undefined || info === undefined) return null;
  return {
    // The alternation is ``` or ~~~, so the first character decides which.
    fence: { char: marker.startsWith("`") ? "`" : "~", len: marker.length },
    info,
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
 * Whether a block whose unindented lines were of `kinds` may be frozen when
 * the next non-blank line of the message is `next`. Everything that could
 * still be pulled into the same construct by what follows answers `false`.
 */
function mayFreeze(kinds: ReadonlySet<BlockKind>, next: string): boolean {
  // Indented: a lazy continuation, an indented code block, or the body of a
  // list item — never a new top-level block.
  if (/^[ \t]/.test(next)) return false;
  // A loose list is one list: items separated by blank lines. The list need
  // not open the block — `Intro:` directly above `- one` is a paragraph and a
  // list in one block — so any item in it keeps a following item with it.
  if (kinds.has("list") && LIST_ITEM.test(next)) return false;
  // Two quoted chunks separated by a blank line are still one blockquote as
  // far as the reader is concerned; keep them together.
  if (kinds.has("quote") && next.startsWith(">")) return false;
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
  // Index into `lines` where the current block starts, and the kinds of the
  // unindented lines seen in that block so far, which decide what a blank
  // line means. Empty until the block has a non-blank line.
  let start = 0;
  let kinds = new Set<BlockKind>();
  let open: Fence | null = null;

  for (let i = 0; i < lines.length; i += 1) {
    const line = lineAt(lines, i);
    if (open) {
      if (closes(open, line)) open = null;
      continue;
    }
    const fence = fenceOf(line);
    if (fence) {
      kinds.add(kindOf(line));
      open = fence.fence;
      continue;
    }
    if (!isBlank(line)) {
      kinds.add(kindOf(line));
      continue;
    }
    // A blank line outside a fence. It is a split point only once further
    // text has arrived — a block is frozen after a blank line *followed by*
    // text, never at the end of what has been received so far.
    let next = i + 1;
    while (next < lines.length && isBlank(lineAt(lines, next))) next += 1;
    if (next >= lines.length) break;
    // Leading blank lines: there is nothing yet to freeze, and they stay with
    // the block that follows them.
    if (kinds.size === 0) continue;
    if (!mayFreeze(kinds, lineAt(lines, next))) continue;
    // The blank run belongs to the block that ends, so that joining the
    // blocks reproduces the message byte for byte.
    blocks.push(lines.slice(start, next).join("\n") + "\n");
    start = next;
    kinds = new Set();
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
