// `SPEC.md`, "Frontend", "Markdown": what is rendered from agent- and
// human-written text, and — more to the point — what is not. Every case here
// that says "no" is the reason the component has no `rehype-raw` and no `img`
// (ADR 0040).

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { MarkdownBody } from "./Markdown";
import KITCHEN_SINK from "./fixtures/markdown-kitchen-sink.md?raw";

afterEach(() => {
  cleanup();
});

function markdown(source: string): HTMLElement {
  const { container } = render(<MarkdownBody>{source}</MarkdownBody>);
  return container;
}

describe("MarkdownBody", () => {
  it("renders a GFM table with header and body cells", () => {
    const container = markdown("| a | b |\n| :- | -: |\n| 1 | 2 |\n");

    expect(container.querySelectorAll("table")).toHaveLength(1);
    expect(
      [...container.querySelectorAll("th")].map((th) => th.textContent),
    ).toEqual(["a", "b"]);
    expect(
      [...container.querySelectorAll("td")].map((td) => td.textContent),
    ).toEqual(["1", "2"]);
  });

  it("honours GFM column alignment and scrolls a wide table inside its own box", () => {
    const container = markdown("| a | b |\n| :-: | -: |\n| 1 | 2 |\n");

    const [left, right] = [...container.querySelectorAll("th")];
    expect(left.style.textAlign).toBe("center");
    expect(right.style.textAlign).toBe("right");

    const wrapper = container.querySelector("table")?.parentElement;
    expect(wrapper?.className).toContain("overflow-x-auto");
  });

  it("renders a task list as disabled checkboxes with no bullet", () => {
    const container = markdown("- [x] done\n- [ ] not done\n");

    const boxes = screen.getAllByRole("checkbox");
    expect(boxes).toHaveLength(2);
    expect(boxes.map((box) => (box as HTMLInputElement).checked)).toEqual([
      true,
      false,
    ]);
    expect(boxes.every((box) => (box as HTMLInputElement).disabled)).toBe(true);
    expect(
      [...container.querySelectorAll("li")].every((li) =>
        li.className.includes("list-none"),
      ),
    ).toBe(true);
  });

  it("renders strikethrough as del", () => {
    const container = markdown("~~gone~~\n");

    expect(container.querySelector("del")?.textContent).toBe("gone");
  });

  it("maps heading levels to h1 through h6", () => {
    const container = markdown(
      "# one\n\n## two\n\n### three\n\n#### four\n\n##### five\n\n###### six\n",
    );

    expect(
      ["h1", "h2", "h3", "h4", "h5", "h6"].map(
        (tag) => container.querySelector(tag)?.textContent,
      ),
    ).toEqual(["one", "two", "three", "four", "five", "six"]);
  });

  it("leaves intraword underscores and asterisks literal", () => {
    const container = markdown(
      "a snake_case_name and 2 * 3 * 4 and a_b_c\n",
    );

    expect(container.querySelectorAll("em")).toHaveLength(0);
    expect(container.querySelectorAll("strong")).toHaveLength(0);
    expect(container.textContent).toContain("snake_case_name");
    expect(container.textContent).toContain("2 * 3 * 4");
  });

  it("distinguishes inline code from a fenced block", () => {
    const container = markdown("an `inline` token\n\n```sh\nblock\n```\n");

    const [inline, block] = [...container.querySelectorAll("code")];
    expect(inline.className).toContain("bg-console-raised");
    expect(inline.closest("pre")).toBeNull();
    expect(block.className).not.toContain("bg-console-raised");
    expect(block.closest("pre")).not.toBeNull();
  });

  it("styles a code fence inside a blockquote as a block", () => {
    const container = markdown("> quoted\n>\n> ```sh\n> echo hi\n> ```\n");

    const code = container.querySelector("blockquote pre code");
    expect(code).not.toBeNull();
    expect(code?.className).not.toContain("bg-console-raised");
  });

  it("does not turn HTML in the source into elements", () => {
    const container = markdown(
      '<script>alert("no")</script>\n\n<img src="https://example.invalid/p.png" onerror="alert(1)">\n',
    );

    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    // Inert: the tag is text on the screen, never an element with a handler.
    expect(container.querySelector("[onerror]")).toBeNull();
    expect(container.textContent).toContain('<script>alert("no")</script>');
  });

  it("never loads an image: an image becomes a link with its alt text", () => {
    const container = markdown("![a pixel](https://example.invalid/y.png)\n");

    expect(container.querySelector("img")).toBeNull();
    const link = container.querySelector("a");
    expect(link?.textContent).toBe("[image: a pixel]");
    expect(link?.getAttribute("href")).toBe("https://example.invalid/y.png");
  });

  it("drops a javascript: link through the default urlTransform", () => {
    const container = markdown("[x](javascript:alert(1))\n");

    const link = container.querySelector("a");
    expect(link?.textContent).toBe("x");
    expect(link?.getAttribute("href") ?? "").not.toContain("javascript:");
  });

  it("opens an external link in a new tab, with noopener", () => {
    const container = markdown("[docs](https://example.invalid/docs)\n");

    const link = container.querySelector("a");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("keeps a footnote anchor in the document instead of navigating away", () => {
    const container = markdown("text[^n]\n\n[^n]: the note\n");

    const ref = container.querySelector("sup a");
    expect(ref?.getAttribute("href")).toBe("#user-content-fn-n");
    expect(ref?.getAttribute("target")).toBeNull();
    // The anchor it points at is still in the tree: the link scrolls, it does
    // not become a route.
    expect(container.querySelector("#user-content-fn-n")).not.toBeNull();
  });

  it("renders the kitchen sink fixture without loading anything", () => {
    const container = markdown(KITCHEN_SINK);

    expect(container.querySelector("table")).not.toBeNull();
    expect(container.querySelector("del")).not.toBeNull();
    expect(container.querySelector("blockquote")).not.toBeNull();
    expect(container.querySelector("hr")).not.toBeNull();
    expect(container.querySelector("ul ul")).not.toBeNull();
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.innerHTML).not.toContain("javascript:");
  });
});
