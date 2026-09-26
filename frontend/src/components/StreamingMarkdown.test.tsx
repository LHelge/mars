// `StreamingMarkdown` (`SPEC.md`, "Frontend", "Markdown"): what the reader
// sees while a message is still being written. The properties under test are
// the ones a reader would notice — no flicker, no torn construct, the same
// document at the end as if it had arrived at once — and the one they would
// not: the frozen blocks are the same DOM nodes from one delta to the next.

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import KITCHEN_SINK from "./fixtures/markdown-kitchen-sink.md?raw";
import { StreamingMarkdown } from "./StreamingMarkdown";

/** Animation frames, run by hand so a test decides when one happens. */
let frames: FrameRequestCallback[] = [];

function flushFrames(): void {
  const due = frames;
  frames = [];
  act(() => {
    for (const frame of due) frame(0);
  });
}

beforeEach(() => {
  frames = [];
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    frames.push(cb);
    return frames.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {});
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function view(text: string, streaming = true) {
  const result = render(
    <StreamingMarkdown streaming={streaming}>{text}</StreamingMarkdown>,
  );
  return {
    container: result.container,
    /** Delivers a delta and lets the frame it books run. */
    feed: (next: string, stillStreaming = true) => {
      act(() => {
        result.rerender(
          <StreamingMarkdown streaming={stillStreaming}>
            {next}
          </StreamingMarkdown>,
        );
      });
      flushFrames();
    },
  };
}

describe("StreamingMarkdown", () => {
  it("coalesces deltas to one render per frame without dropping text", () => {
    const { container, ...rest } = render(
      <StreamingMarkdown streaming>a</StreamingMarkdown>,
    );
    for (const text of ["ab", "abc", "abcd"]) {
      act(() => {
        rest.rerender(<StreamingMarkdown streaming>{text}</StreamingMarkdown>);
      });
    }

    // Four deltas, one frame booked, and nothing shown yet but the first.
    expect(frames).toHaveLength(1);
    expect(container.textContent).toBe("a");

    flushFrames();

    // The frame renders what arrived while it was pending, not the delta that
    // booked it: no text is lost and nothing in between was ever painted.
    expect(container.textContent).toBe("abcd");
    expect(frames).toHaveLength(0);
  });

  it("renders a completed message at once, without waiting for a frame", () => {
    const { container, rerender } = render(
      <StreamingMarkdown streaming>done so f</StreamingMarkdown>,
    );
    act(() => {
      rerender(
        <StreamingMarkdown streaming={false}>done so far</StreamingMarkdown>,
      );
    });

    expect(container.textContent).toBe("done so far");
  });

  it("renders an unclosed fence as one code block", () => {
    const { container, feed } = view("Here:\n\n```sh\necho one\n");

    expect(container.querySelectorAll("pre")).toHaveLength(1);
    expect(container.querySelector("pre")?.textContent).toContain("echo one");

    // A blank line inside the open fence does not end it.
    feed("Here:\n\n```sh\necho one\n\necho two\n");
    expect(container.querySelectorAll("pre")).toHaveLength(1);
    expect(container.querySelector("pre")?.textContent).toContain("echo two");
    expect(container.querySelectorAll("p")).toHaveLength(1);
  });

  it("shows a table without its delimiter row as text and turns it into a table once", () => {
    const { container, feed } = view("| a | b |\n");

    expect(container.querySelectorAll("table")).toHaveLength(0);
    expect(container.textContent).toContain("| a | b |");

    feed("| a | b |\n| - | - |\n");
    expect(container.querySelectorAll("table")).toHaveLength(1);

    const table = container.querySelector("table");
    for (const row of ["| 1 | 2 |\n", "| 3 | 4 |\n"]) {
      feed(`| a | b |\n| - | - |\n${row.slice(0, 4)}`);
      // Still one table, and the same one: it does not alternate between a
      // paragraph and a table as the half-written row grows.
      expect(container.querySelectorAll("table")).toHaveLength(1);
      expect(container.querySelector("table")).toBe(table);
    }
  });

  it("keeps the frozen blocks' DOM nodes across deltas", () => {
    const { container, feed } = view("# Title\n\nfirst paragraph\n\nsec");
    const frozen = [...container.children].slice(0, -1);
    expect(frozen.length).toBeGreaterThan(0);

    feed("# Title\n\nfirst paragraph\n\nsecond paragraph\n\nthi");
    feed("# Title\n\nfirst paragraph\n\nsecond paragraph\n\nthird");

    expect([...container.children].slice(0, frozen.length)).toEqual(frozen);
  });

  it("keeps a heading's top margin when it starts a block of its own", () => {
    const { container } = view("intro\n\n## Later\n\nbody");

    // `first:mt-0` would otherwise eat the margin of every block's first
    // element, and a heading mid-message would sit on the text above it.
    const later = container.querySelector("h2");
    expect(later?.parentElement?.className).toContain(
      "[&>h2:first-child]:mt-3",
    );
    // The first block keeps the reset: a message does not start with a gap.
    expect(container.firstElementChild?.className).not.toContain(
      "first-child]:mt-3",
    );
  });

  it("ends a character-by-character stream in the same DOM as one render", () => {
    const { container, rerender } = render(
      <StreamingMarkdown streaming>
        {KITCHEN_SINK.slice(0, 1)}
      </StreamingMarkdown>,
    );
    for (let i = 2; i <= KITCHEN_SINK.length; i += 1) {
      act(() => {
        rerender(
          <StreamingMarkdown streaming>
            {KITCHEN_SINK.slice(0, i)}
          </StreamingMarkdown>,
        );
      });
      flushFrames();
      // No prefix of the message ever turns HTML in the text into elements.
      expect(container.querySelector("script")).toBeNull();
      expect(container.querySelector("img")).toBeNull();
    }
    act(() => {
      rerender(
        <StreamingMarkdown streaming={false}>{KITCHEN_SINK}</StreamingMarkdown>,
      );
    });

    const atOnce = render(
      <StreamingMarkdown streaming={false}>{KITCHEN_SINK}</StreamingMarkdown>,
    );
    expect(container.innerHTML).toBe(atOnce.container.innerHTML);
    // One render per character of the fixture is seconds of CPU on a quiet
    // machine, and Vitest's 5 s default does not survive a loaded one.
  }, 30_000);
});
