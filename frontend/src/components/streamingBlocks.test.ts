// The block splitter behind `StreamingMarkdown` (`SPEC.md`, "Frontend",
// "Markdown"). Two invariants run through every case: the blocks join back
// into the input byte for byte, and no construct is ever torn apart — when
// the splitter is unsure it leaves the text in the growing tail.

import { describe, expect, it } from "vitest";

import KITCHEN_SINK from "./fixtures/markdown-kitchen-sink.md?raw";
import {
  hasReferenceDefinitions,
  splitTopLevelBlocks,
} from "./streamingBlocks";

function split(text: string): string[] {
  const blocks = splitTopLevelBlocks(text);
  // The invariant the whole design rests on: the reader sees the same text.
  expect(blocks.join("")).toBe(text);
  return blocks;
}

describe("splitTopLevelBlocks", () => {
  it("freezes a paragraph once text follows the blank line", () => {
    expect(split("one\n\ntwo")).toEqual(["one\n\n", "two"]);
  });

  it("keeps a trailing blank line in the tail until text follows it", () => {
    expect(split("one\n\n")).toEqual(["one\n\n"]);
    expect(split("one")).toEqual(["one"]);
  });

  it("never splits inside a fence, whichever character opened it", () => {
    for (const marker of ["```", "~~~~"]) {
      expect(
        split(`${marker}rust\nfn a() {}\n\nfn b() {}\n${marker}\n\nafter`),
      ).toEqual([
        `${marker}rust\nfn a() {}\n\nfn b() {}\n${marker}\n\n`,
        "after",
      ]);
    }
  });

  it("keeps an unclosed fence in one block to the end of the text", () => {
    expect(split("intro\n\n```sh\necho one\n\necho two\n")).toEqual([
      "intro\n\n",
      "```sh\necho one\n\necho two\n",
    ]);
  });

  it("closes a fence only on a longer or equal run of the same character", () => {
    expect(split("````\n``\n\nstill code\n````\n\nafter")).toEqual([
      "````\n``\n\nstill code\n````\n\n",
      "after",
    ]);
    expect(split("```\n~~~\n\nstill code\n```\n\nafter")).toEqual([
      "```\n~~~\n\nstill code\n```\n\n",
      "after",
    ]);
  });

  it("keeps a loose list together", () => {
    expect(split("- one\n\n- two\n\n- three")).toEqual([
      "- one\n\n- two\n\n- three",
    ]);
    expect(split("1. one\n\n2. two\n\nafter")).toEqual([
      "1. one\n\n2. two\n\n",
      "after",
    ]);
    // A list that starts under an intro line, in the same block, is still
    // one loose list.
    expect(split("Intro:\n- one\n\n- two\n\nafter")).toEqual([
      "Intro:\n- one\n\n- two\n\n",
      "after",
    ]);
  });

  it("keeps a blockquote with a blank line together", () => {
    expect(split("> one\n\n> two\n\nafter")).toEqual([
      "> one\n\n> two\n\n",
      "after",
    ]);
  });

  it("keeps indented continuations and indented code with their block", () => {
    expect(split("- item\n\n  still the item\n\nafter")).toEqual([
      "- item\n\n  still the item\n\n",
      "after",
    ]);
    // The blank line before an indented block is not a boundary, so the
    // paragraph is frozen late, with the code: a late split is only slower.
    expect(
      split("para\n\n    indented code\n\n    more code\n\nafter"),
    ).toEqual(["para\n\n    indented code\n\n    more code\n\n", "after"]);
  });

  it("does not split a fence that sits inside a quote or a list item", () => {
    expect(split("> ```sh\n> echo hi\n\n> ```\n\nafter")).toEqual([
      "> ```sh\n> echo hi\n\n> ```\n\n",
      "after",
    ]);
  });

  it("treats a message with no blank line as one block", () => {
    const long = "word ".repeat(2000);
    expect(split(long)).toEqual([long]);
  });

  it("gives leading blank lines to the block that follows", () => {
    expect(split("\n\none\n\ntwo")).toEqual(["\n\none\n\n", "two"]);
    expect(split("\n\n")).toEqual(["\n\n"]);
    expect(split("")).toEqual([""]);
  });

  it("splits the kitchen sink into blocks that join back together", () => {
    const blocks = split(KITCHEN_SINK);

    expect(blocks.length).toBeGreaterThan(10);
    // The fenced Rust block is one block, braces and all.
    expect(
      blocks.some(
        (block) =>
          block.startsWith("```rust") && block.includes('println!("hello")'),
      ),
    ).toBe(true);
    // The table is one block: header, delimiter row and every body row.
    const table = blocks.find((block) => block.startsWith("| Left |"));
    expect(table).toBeDefined();
    expect(table).toContain("| :--- | :----: | ----: |");
    expect(table).toContain("| a cell that is very much longer");
  });

  it("splits every prefix of the kitchen sink without losing text", () => {
    for (let i = 0; i <= KITCHEN_SINK.length; i += 1) {
      const prefix = KITCHEN_SINK.slice(0, i);
      expect(splitTopLevelBlocks(prefix).join("")).toBe(prefix);
    }
  });

  it("only ever appends blocks as the text grows", () => {
    let previous: string[] = [];
    for (let i = 0; i <= KITCHEN_SINK.length; i += 1) {
      const blocks = splitTopLevelBlocks(KITCHEN_SINK.slice(0, i));
      // A frozen block never changes and never moves: every block but the
      // growing tail is exactly what it was one character earlier.
      for (let b = 0; b < blocks.length - 1; b += 1) {
        if (b < previous.length - 1) expect(blocks[b]).toBe(previous[b]);
      }
      previous = blocks;
    }
  });
});

describe("hasReferenceDefinitions", () => {
  it("finds footnote and link reference definitions", () => {
    expect(hasReferenceDefinitions("a[^n]\n\n[^n]: the note\n")).toBe(true);
    expect(
      hasReferenceDefinitions("a [b][ref]\n\n[ref]: https://x.invalid\n"),
    ).toBe(true);
    expect(hasReferenceDefinitions(KITCHEN_SINK)).toBe(true);
  });

  it("does not see a definition in ordinary text or a link", () => {
    expect(hasReferenceDefinitions("a [link](https://x.invalid) here")).toBe(
      false,
    );
    expect(hasReferenceDefinitions("- [x] done\n- [ ] not done\n")).toBe(false);
  });
});
