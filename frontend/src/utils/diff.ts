// Line diffs for the transcript and the Changes panel (`SPEC.md`, "Transcript
// rendering" and "Changes panel").
//
// Two producers, one shape: an edit tool hands over the two versions of a
// string and `lineDiff` aligns them; git hands over a unified patch and
// `parseUnifiedPatch` reads the alignment git already computed. Both answer
// `DiffLine[]`, which is what `DiffView` draws. No diff dependency: the
// alignment is a longest-common-subsequence walk over lines.

import { splitLines } from "./lines";

/** One rendered row of a diff, with the line numbers it carries on each side. */
export interface DiffLine {
  type: "context" | "add" | "del";
  text: string;
  /** 1-based line number in the old text; absent on an addition. */
  oldNo?: number;
  /** 1-based line number in the new text; absent on a deletion. */
  newNo?: number;
}

export interface PatchHunk {
  /** The `@@ ... @@` line verbatim, section heading included. */
  header: string;
  lines: DiffLine[];
}

export interface PatchFile {
  path: string;
  hunks: PatchHunk[];
  /**
   * git refused to diff this file's contents, so it has no hunks and no line
   * counts. The Changes panel reads it to write `bin` where the counts go: the
   * endpoint reports zero additions and zero deletions for a binary file
   * (`SPEC.md`, "Git"), which is also what a mode-only change reports, so the
   * patch is the only place that tells the two apart.
   */
  binary: boolean;
}

/** Above this many lines on either side the alignment is not attempted. */
export const DIFF_LINE_CAP = 5000;

// The alignment table is O(n·m) cells, so a pair that is large on both sides
// and shares no context is refused even under the line cap: a browser tab is
// not the place to spend a hundred megabytes aligning two unrelated files.
const DIFF_CELL_CAP = 1_000_000;

/** The shared head and tail, which never need aligning. */
function commonRange(a: string[], b: string[]) {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) {
    start += 1;
  }
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  return { start, endA, endB };
}

function overCap(a: string[], b: string[]): boolean {
  if (a.length > DIFF_LINE_CAP || b.length > DIFF_LINE_CAP) {
    return true;
  }
  const { start, endA, endB } = commonRange(a, b);
  return (endA - start) * (endB - start) > DIFF_CELL_CAP;
}

/**
 * True when `lineDiff` would give up on alignment and fall back to an "old"
 * block followed by a "new" block. Callers use it to show the notice that says
 * so; `DiffView` has no way to tell the two results apart on its own.
 */
export function lineDiffCapped(oldText: string, newText: string): boolean {
  return overCap(splitLines(oldText), splitLines(newText));
}

/**
 * The line at `index`, or `""` for one outside the array.
 *
 * Every call below is inside its own bound — the walk never leaves the aligned
 * middle it computed — so the fallback is unreachable rather than a guess. It
 * is here so the walk reads as the arithmetic it is instead of as a chain of
 * undefined checks, and so an off-by-one renders a blank line rather than the
 * word `undefined`.
 */
function lineAt(lines: string[], index: number): string {
  return lines[index] ?? "";
}

/**
 * One cell of the suffix-LCS table. The table is `(n + 1) x (m + 1)` with a
 * zero row and column at the end, and `0` is exactly what a read past them
 * means: no common suffix remains.
 */
function cell(lcs: Uint32Array, index: number): number {
  return lcs[index] ?? 0;
}

function unaligned(a: string[], b: string[]): DiffLine[] {
  return [
    ...a.map((text, i): DiffLine => ({ type: "del", text, oldNo: i + 1 })),
    ...b.map((text, i): DiffLine => ({ type: "add", text, newNo: i + 1 })),
  ];
}

/**
 * The line-by-line difference between two texts, in reading order: context
 * lines carry both line numbers, a deletion only the old one and an addition
 * only the new one. An empty `oldText` therefore renders as a pure insertion.
 */
export function lineDiff(oldText: string, newText: string): DiffLine[] {
  const a = splitLines(oldText);
  const b = splitLines(newText);
  if (overCap(a, b)) {
    return unaligned(a, b);
  }

  const { start, endA, endB } = commonRange(a, b);
  const n = endA - start;
  const m = endB - start;

  // Suffix lengths of the longest common subsequence, filled from the end so
  // the walk below can read it forward and keep the diff in reading order.
  const width = m + 1;
  const lcs = new Uint32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      lcs[i * width + j] =
        lineAt(a, start + i) === lineAt(b, start + j)
          ? cell(lcs, (i + 1) * width + j + 1) + 1
          : Math.max(
              cell(lcs, (i + 1) * width + j),
              cell(lcs, i * width + j + 1),
            );
    }
  }

  const lines: DiffLine[] = [];
  for (let k = 0; k < start; k += 1) {
    lines.push({
      type: "context",
      text: lineAt(a, k),
      oldNo: k + 1,
      newNo: k + 1,
    });
  }

  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (lineAt(a, start + i) === lineAt(b, start + j)) {
      lines.push({
        type: "context",
        text: lineAt(a, start + i),
        oldNo: start + i + 1,
        newNo: start + j + 1,
      });
      i += 1;
      j += 1;
    } else if (cell(lcs, (i + 1) * width + j) >= cell(lcs, i * width + j + 1)) {
      lines.push({
        type: "del",
        text: lineAt(a, start + i),
        oldNo: start + i + 1,
      });
      i += 1;
    } else {
      lines.push({
        type: "add",
        text: lineAt(b, start + j),
        newNo: start + j + 1,
      });
      j += 1;
    }
  }
  for (; i < n; i += 1) {
    lines.push({
      type: "del",
      text: lineAt(a, start + i),
      oldNo: start + i + 1,
    });
  }
  for (; j < m; j += 1) {
    lines.push({
      type: "add",
      text: lineAt(b, start + j),
      newNo: start + j + 1,
    });
  }

  for (let k = 0; k < a.length - endA; k += 1) {
    lines.push({
      type: "context",
      text: lineAt(a, endA + k),
      oldNo: endA + k + 1,
      newNo: endB + k + 1,
    });
  }
  return lines;
}

// The counts a hunk header declares. Both are optional and an omitted count
// means 1 (`@@ -3 +3 @@`), which is what makes the counts usable as the
// authority for where the hunk body ends: a patch line is body while the hunk
// still owes lines and a header only once it does not.
const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** The prefix of a `diff --git` header, the two paths excluded. */
const GIT_HEADER = "diff --git ";

/** The single-character escapes git writes in a quoted path, as byte values. */
const ESCAPES = new Map<string, number>([
  ["a", 0x07],
  ["b", 0x08],
  ["f", 0x0c],
  ["n", 0x0a],
  ["r", 0x0d],
  ["t", 0x09],
  ["v", 0x0b],
  ["\\", 0x5c],
  ['"', 0x22],
]);

/**
 * A structural line without the `\r` a CRLF-terminated patch leaves on it.
 *
 * Only headers are read through this. A `\r` at the end of a *content* line is
 * the file's own line ending and is kept, because nothing in the patch tells
 * the two cases apart and a CRLF file must not render as if it were LF. No
 * unquoted git path ends in `\r` — git quotes a path with a control character
 * in it — so stripping one here cannot eat part of a name.
 */
function withoutCr(line: string): string {
  return line.endsWith("\r") ? line.slice(0, -1) : line;
}

/**
 * A C-quoted path as git writes it when `core.quotePath` is on or the name
 * carries a control character: `"a/f\303\266o.png"` back to `a/föo.png`.
 *
 * The escapes are undone into bytes and the bytes decoded as UTF-8, because an
 * octal escape is one byte of a name and not one character: undoing them one
 * at a time into code points would turn `\303\266` into two.
 *
 * Anything that is not a quoted token is returned unchanged, so an ordinary
 * path may be passed through this on its way to the file list.
 */
function unquotePath(raw: string): string {
  if (raw.length < 2 || !raw.startsWith('"') || !raw.endsWith('"')) {
    return raw;
  }

  const body = raw.slice(1, -1);
  const encoder = new TextEncoder();
  const bytes: number[] = [];
  const pushText = (text: string) => {
    for (const byte of encoder.encode(text)) {
      bytes.push(byte);
    }
  };

  let at = 0;
  while (at < body.length) {
    const escape = body.indexOf("\\", at);
    if (escape === -1) {
      pushText(body.slice(at));
      break;
    }
    // Sliced rather than walked character by character, so a surrogate pair in
    // the literal part survives the round trip through the encoder.
    pushText(body.slice(at, escape));

    const next = body[escape + 1];
    if (next === undefined) {
      pushText("\\");
      break;
    }
    const single = ESCAPES.get(next);
    if (single !== undefined) {
      bytes.push(single);
      at = escape + 2;
      continue;
    }
    const octal = /^[0-7]{1,3}/.exec(body.slice(escape + 1, escape + 4));
    if (octal) {
      bytes.push(Number.parseInt(octal[0], 8) & 0xff);
      at = escape + 1 + octal[0].length;
      continue;
    }
    // An escape git does not write: keep the character it introduced.
    pushText(next);
    at = escape + 2;
  }

  return new TextDecoder().decode(new Uint8Array(bytes));
}

/** The file a `+++ b/path` or `--- a/path` header names, prefix stripped. */
function headerPath(raw: string): string | null {
  const path = unquotePath(raw.trim());
  if (path === "" || path === "/dev/null") {
    return null;
  }
  return /^[ab]\//.test(path) ? path.slice(2) : path;
}

/** The index of the closing quote of the quoted token `text` starts with. */
function quotedEnd(text: string): number {
  for (let i = 1; i < text.length; i += 1) {
    if (text[i] === "\\") {
      i += 1;
      continue;
    }
    if (text[i] === '"') {
      return i;
    }
  }
  return -1;
}

/**
 * The `b/` path of a `diff --git` header, from everything after the prefix.
 *
 * Either side may be quoted and either side may contain spaces, so the second
 * token is found by walking the first rather than by splitting on whitespace:
 * a quoted first token ends at its closing quote, and a bare one ends at the
 * ` b/` that starts the second.
 */
function gitHeaderPath(rest: string): string | null {
  if (rest.startsWith('"')) {
    const end = quotedEnd(rest);
    if (end === -1 || rest[end + 1] !== " ") {
      return null;
    }
    return headerPath(rest.slice(end + 2));
  }
  const bare = /^a\/.+? ("b\/(?:[^"\\]|\\.)*"|b\/.+)$/.exec(rest);
  const second = bare?.[1];
  return second === undefined ? null : headerPath(second);
}

/**
 * Both spellings of "this file has no readable diff": the one-line notice of a
 * plain `git diff` and the header of a `--binary` patch.
 */
function isBinaryNotice(line: string): boolean {
  return (
    (line.startsWith("Binary files ") && line.endsWith(" differ")) ||
    line === "GIT binary patch"
  );
}

/**
 * A unified patch as the git diff endpoint returns it, split per file and per
 * hunk. Lines git could not attribute to a file (`index`, mode changes) are
 * dropped: the panel lists what changed, not how git said it. A binary notice
 * is the exception — it carries the only signal that a file has no line counts,
 * so it sets `binary` instead of being dropped.
 *
 * **Counts decide, not prefixes.** A hunk body is read for exactly the old and
 * new line counts its `@@` header declares, and only a line outside a hunk can
 * be a header. Content is what a patch is made of, so content that looks like
 * a header is the ordinary case rather than the strange one: deleting the SQL
 * comment `-- old` sends `--- old` and adding `++ b` sends `+++ b`, and a
 * parser that tested for the prefix first swallowed the one and renamed the
 * file on the other.
 *
 * A file is named by the `b/` side of its `diff --git` header, unquoted when
 * git quoted it, so a rename appears under its new path and a non-ASCII name
 * matches the path the file list carries. The diff endpoint passes
 * `--no-renames` and `-c core.quotePath=false` (`ARCHITECTURE.md`, "Git
 * model"), so in practice a rename arrives as a deletion and an addition and a
 * quoted path only from somewhere else; both still parse. A `diff --git` line
 * whose paths cannot be read still starts a new file, so the `Binary files …
 * differ` under it can never mark the file above it as binary.
 */
export function parseUnifiedPatch(patch: string): PatchFile[] {
  const files: PatchFile[] = [];
  let file: PatchFile | null = null;
  let hunk: PatchHunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  // What the open hunk's header still owes on each side.
  let oldLeft = 0;
  let newLeft = 0;

  // Assigned to `file` by the caller rather than from in here, so the
  // narrowing of `file` below stays the compiler's to do.
  const opened = (path: string): PatchFile => {
    const next: PatchFile = { path, hunks: [], binary: false };
    files.push(next);
    return next;
  };

  // A patch ends with a newline, which `split` turns into a trailing empty
  // element; it is the line terminator, not a blank context line.
  const source = patch.split("\n");
  if (source[source.length - 1] === "") {
    source.pop();
  }

  for (const raw of source) {
    if (hunk !== null && (oldLeft > 0 || newLeft > 0)) {
      if (raw.startsWith("\\")) {
        // "\ No newline at end of file": metadata about the line before it,
        // and not one of the lines the header counted.
        continue;
      }
      if (raw.startsWith("+")) {
        hunk.lines.push({ type: "add", text: raw.slice(1), newNo });
        newNo += 1;
        newLeft -= 1;
        continue;
      }
      if (raw.startsWith("-")) {
        hunk.lines.push({ type: "del", text: raw.slice(1), oldNo });
        oldNo += 1;
        oldLeft -= 1;
        continue;
      }
      if (raw.startsWith(" ") || raw === "") {
        hunk.lines.push({ type: "context", text: raw.slice(1), oldNo, newNo });
        oldNo += 1;
        newNo += 1;
        oldLeft -= 1;
        newLeft -= 1;
        continue;
      }
      // The hunk declared more lines than it carries: a truncated or
      // hand-edited patch. End it here and read this line as a header.
      hunk = null;
      oldLeft = 0;
      newLeft = 0;
    }

    const line = withoutCr(raw);

    if (line.startsWith(GIT_HEADER)) {
      const rest = line.slice(GIT_HEADER.length);
      file = opened(gitHeaderPath(rest) ?? rest);
      hunk = null;
      oldLeft = 0;
      newLeft = 0;
      continue;
    }
    if (line.startsWith("--- ")) {
      const path = headerPath(line.slice(4));
      // A `---` under a file that already has hunks is the next file of a
      // plain `diff -u` patch, which has no `diff --git` line to start it. In
      // a git patch the file is always still empty here, so this never fires.
      if (path !== null && (file === null || file.hunks.length > 0)) {
        file = opened(path);
      }
      continue;
    }
    if (line.startsWith("+++ ")) {
      const path = headerPath(line.slice(4));
      if (path !== null) {
        if (file === null) {
          file = opened(path);
        } else {
          file.path = path;
        }
      }
      continue;
    }

    const bounds = HUNK.exec(line);
    if (bounds) {
      if (file === null) {
        file = opened("");
      }
      oldNo = Number(bounds[1]);
      newNo = Number(bounds[3]);
      // An omitted count means one line, which is how git writes a one-line
      // side (`@@ -3 +3 @@`).
      oldLeft = bounds[2] === undefined ? 1 : Number(bounds[2]);
      newLeft = bounds[4] === undefined ? 1 : Number(bounds[4]);
      hunk = { header: line, lines: [] };
      file.hunks.push(hunk);
      continue;
    }

    if (file !== null && isBinaryNotice(line)) {
      file.binary = true;
    }
  }

  return files;
}
