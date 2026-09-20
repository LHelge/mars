// How the transcript prints a value it has no dedicated renderer for.
//
// `SPEC.md`, "Transcript rendering", asks for a JSON tree for anything that is
// not a known tool family and for `raw`. The tree itself is the next task; until
// it lands every such value is pretty-printed JSON in the mono face, which is
// the same content in the same place, just flat.

/**
 * A value as the transcript shows it: a string stays the string the agent
 * produced, anything else becomes indented JSON. Never throws: a cyclic or
 * otherwise unserialisable value falls back to its `String` form, because a
 * transcript row must always render something.
 */
export function formatValue(value: unknown): string {
  if (value === undefined || value === null) {
    return "";
  }
  if (typeof value === "string") {
    return value;
  }
  try {
    // `JSON.stringify` answers `undefined` for a function or a symbol, which
    // no event payload holds; there is nothing to show for one either way.
    return JSON.stringify(value, null, 2) ?? "";
  } catch {
    return "[unserialisable]";
  }
}

/** The number of lines `formatValue` would print, used by the 40-line rule. */
export function lineCount(text: string): number {
  return text === "" ? 0 : text.split("\n").length;
}
