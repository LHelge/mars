// Counting the lines of a blob, in one place.
//
// Its own module rather than a corner of `diff.ts`: the diff alignment and the
// patch parser are only ever reached from a lazy route chunk, and a component
// of the main chunk that only needs the line count must not drag them along
// (`SPEC.md`, "Frontend", "Code splitting").

/**
 * The lines of `text`. A trailing newline ends the last line rather than
 * starting an empty one, so a file and its content render the same number of
 * rows an editor shows and a 40-line blob is not reported as 41.
 */
export function splitLines(text: string): string[] {
  if (text === "") {
    return [];
  }
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines;
}
