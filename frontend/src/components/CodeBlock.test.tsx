// `SPEC.md`, "Frontend", "Markdown": a fence is readable before the
// highlighter exists, `Copy` writes what was fenced and nothing else, and the
// two cases where colouring is skipped — an unknown tag and a block the size
// of a log — stay plain for good.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CodeBlock } from "./CodeBlock";

const RUST = 'fn main() {\n    println!("hi");\n}';

function setClipboard(value: unknown): void {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value });
}

beforeEach(() => {
  setClipboard(undefined);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("CodeBlock", () => {
  it("renders the code as plain text before the highlighter chunk resolves", () => {
    const { container } = render(<CodeBlock code={RUST} language="rust" />);

    const code = container.querySelector("code");
    expect(code?.textContent).toBe(RUST);
    expect(container.querySelectorAll("code span")).toHaveLength(0);
  });

  it("upgrades to highlighted spans once the chunk has loaded", async () => {
    const { container } = render(<CodeBlock code={RUST} language="rust" />);

    await waitFor(() => {
      expect(container.querySelector("code .hljs-keyword")).not.toBeNull();
    });
    // The upgrade changes the markup, never the text.
    expect(container.querySelector("code")?.textContent).toBe(RUST);
  });

  it("highlights through an alias highlight.js declares itself", async () => {
    const { container } = render(<CodeBlock code={RUST} language="rs" />);

    await waitFor(() => {
      expect(container.querySelector("code .hljs-keyword")).not.toBeNull();
    });
  });

  it("shows an unknown language on the header and leaves the body plain", async () => {
    const { container } = render(
      <CodeBlock code={RUST} language="brainfuck" />,
    );

    expect(screen.getByText("brainfuck")).toBeDefined();
    // Long enough for the settle timer and the import to have had their turn.
    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(container.querySelectorAll("code span")).toHaveLength(0);
    expect(container.querySelector("code")?.textContent).toBe(RUST);
  });

  it("leaves a fence with no language plain", async () => {
    const { container } = render(<CodeBlock code={RUST} />);

    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(container.querySelectorAll("code span")).toHaveLength(0);
  });

  it("does not highlight a block above the line limit", async () => {
    const many = `${"let x = 1;\n".repeat(2001)}let y = 2;`;
    const { container } = render(<CodeBlock code={many} language="rust" />);

    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(container.querySelectorAll("code span")).toHaveLength(0);
  });

  it("does not highlight a block above the size limit", async () => {
    const huge = `// ${"x".repeat(200 * 1024)}`;
    const { container } = render(<CodeBlock code={huge} language="rust" />);

    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(container.querySelectorAll("code span")).toHaveLength(0);
  });

  it("copies exactly the source and confirms only after the write resolves", async () => {
    let resolve: (() => void) | undefined;
    const writeText = vi.fn<(text: string) => Promise<void>>(
      () =>
        new Promise<void>((done) => {
          resolve = done;
        }),
    );
    setClipboard({ writeText });

    render(<CodeBlock code={RUST} language="rust" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));

    expect(writeText).toHaveBeenCalledWith(RUST);
    expect(screen.queryByRole("button", { name: "Copied" })).toBeNull();

    resolve?.();
    await screen.findByRole("button", { name: "Copied" });
  });

  it("selects the code when the clipboard refuses", async () => {
    setClipboard({
      writeText: vi.fn(() => Promise.reject(new Error("denied"))),
    });

    const { container } = render(<CodeBlock code={RUST} language="rust" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));

    await screen.findByText(/Clipboard unavailable/);
    expect(screen.queryByRole("button", { name: "Copied" })).toBeNull();
    expect(window.getSelection()?.toString()).toBe(
      container.querySelector("code")?.textContent,
    );
  });
});
