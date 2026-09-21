import { describe, expect, it } from "vitest";

import { splitLines } from "./lines";

describe("splitLines", () => {
  it("counts the newline that ends the last line as a terminator", () => {
    expect(splitLines("a\nb\n")).toEqual(["a", "b"]);
    expect(splitLines("a\nb")).toEqual(["a", "b"]);
    expect(splitLines("")).toEqual([]);
    // Only the final terminator is dropped: a blank last line is a line.
    expect(splitLines("a\n\n")).toEqual(["a", ""]);
  });

  it("counts a 40-line blob that ends in a newline as 40 lines", () => {
    // The collapse threshold in `CollapsibleLines`: one line more and the
    // control appears and says how many are hidden.
    const blob = `${Array.from({ length: 40 }, (_, i) => `l${String(i)}`).join("\n")}\n`;
    expect(splitLines(blob)).toHaveLength(40);
  });
});
