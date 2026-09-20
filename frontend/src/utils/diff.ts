// Line diffs for the transcript and the Changes panel (`SPEC.md`, "Transcript
// rendering" and "Changes panel").
//
// Two producers, one shape: an edit tool hands over the two versions of a
// string and `lineDiff` aligns them; git hands over a unified patch and
// `parseUnifiedPatch` reads the alignment git already computed. Both answer
// `DiffLine[]`, which is what `DiffView` draws. No diff dependency: the
// alignment is a longest-common-subsequence walk over lines.

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

/**
 * The lines of `text`. A trailing newline ends the last line rather than
 * starting an empty one, so a file and its content render the same number of
 * rows an editor shows.
 */
function splitLines(text: string): string[] {
  if (text === "") {
    return [];
  }
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines;
}

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
        a[start + i] === b[start + j]
          ? lcs[(i + 1) * width + j + 1] + 1
          : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
    }
  }

  const lines: DiffLine[] = [];
  for (let k = 0; k < start; k += 1) {
    lines.push({ type: "context", text: a[k], oldNo: k + 1, newNo: k + 1 });
  }

  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[start + i] === b[start + j]) {
      lines.push({
        type: "context",
        text: a[start + i],
        oldNo: start + i + 1,
        newNo: start + j + 1,
      });
      i += 1;
      j += 1;
    } else if (lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
      lines.push({ type: "del", text: a[start + i], oldNo: start + i + 1 });
      i += 1;
    } else {
      lines.push({ type: "add", text: b[start + j], newNo: start + j + 1 });
      j += 1;
    }
  }
  for (; i < n; i += 1) {
    lines.push({ type: "del", text: a[start + i], oldNo: start + i + 1 });
  }
  for (; j < m; j += 1) {
    lines.push({ type: "add", text: b[start + j], newNo: start + j + 1 });
  }

  for (let k = 0; k < a.length - endA; k += 1) {
    lines.push({
      type: "context",
      text: a[endA + k],
      oldNo: endA + k + 1,
      newNo: endB + k + 1,
    });
  }
  return lines;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/** The file a `+++ b/path` or `--- a/path` header names, prefix stripped. */
function headerPath(raw: string): string | null {
  const path = raw.trim();
  if (path === "" || path === "/dev/null") {
    return null;
  }
  return /^[ab]\//.test(path) ? path.slice(2) : path;
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
 * A file is named by the `b/` side of its `diff --git` header, so a rename
 * appears under its new path. The diff endpoint passes `--no-renames`
 * (`ARCHITECTURE.md`, "Git model"), so in practice a rename arrives as a
 * deletion and an addition; a patch from anywhere else still parses.
 */
export function parseUnifiedPatch(patch: string): PatchFile[] {
  const files: PatchFile[] = [];
  let file: PatchFile | null = null;
  let hunk: PatchHunk | null = null;
  let oldNo = 0;
  let newNo = 0;

  // A patch ends with a newline, which `split` turns into a trailing empty
  // element; it is the line terminator, not a blank context line.
  const source = patch.split("\n");
  if (source[source.length - 1] === "") {
    source.pop();
  }

  for (const line of source) {
    const git = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
    if (git) {
      file = { path: git[2], hunks: [], binary: false };
      files.push(file);
      hunk = null;
      continue;
    }
    if (line.startsWith("--- ")) {
      const path = headerPath(line.slice(4));
      if (file === null && path !== null) {
        file = { path, hunks: [], binary: false };
        files.push(file);
        hunk = null;
      }
      continue;
    }
    if (line.startsWith("+++ ")) {
      const path = headerPath(line.slice(4));
      if (path !== null) {
        if (file === null) {
          file = { path, hunks: [], binary: false };
          files.push(file);
          hunk = null;
        } else {
          file.path = path;
        }
      }
      continue;
    }

    const bounds = HUNK.exec(line);
    if (bounds) {
      if (file === null) {
        file = { path: "", hunks: [], binary: false };
        files.push(file);
      }
      oldNo = Number(bounds[1]);
      newNo = Number(bounds[2]);
      hunk = { header: line, lines: [] };
      file.hunks.push(hunk);
      continue;
    }

    if (hunk === null) {
      if (file !== null && isBinaryNotice(line)) {
        file.binary = true;
      }
      continue;
    }
    if (line.startsWith("\\")) {
      // "\ No newline at end of file": metadata about the line before it.
      continue;
    }
    if (line.startsWith("+")) {
      hunk.lines.push({ type: "add", text: line.slice(1), newNo });
      newNo += 1;
    } else if (line.startsWith("-")) {
      hunk.lines.push({ type: "del", text: line.slice(1), oldNo });
      oldNo += 1;
    } else if (line.startsWith(" ") || line === "") {
      hunk.lines.push({
        type: "context",
        text: line.slice(1),
        oldNo,
        newNo,
      });
      oldNo += 1;
      newNo += 1;
    } else {
      // Anything else ends the hunk: the next file header, or trailing noise.
      hunk = null;
    }
  }

  return files;
}
