// One render per tool family, over hand-built `ToolMessage`s (`SPEC.md`,
// "Transcript rendering"). The registry is exercised through `ToolFrame`,
// which is how a transcript row reaches a renderer.

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { SubagentGroup } from "../SubagentGroup";
import type { ToolMessage } from "../sessionStore";
import { EditToolRenderer } from "./EditToolRenderer";
import { JsonToolRenderer } from "./JsonToolRenderer";
import { ShellToolRenderer } from "./ShellToolRenderer";
import { SummaryToolRenderer } from "./SummaryToolRenderer";
import { ToolFrame } from "./ToolFrame";

afterEach(cleanup);

function tool(fields: Partial<ToolMessage> & { name: string }): ToolMessage {
  return {
    id: "message-1",
    kind: "tool",
    tool_use_id: "toolu_1",
    input: {},
    running: false,
    ...fields,
  };
}

describe("EditToolRenderer", () => {
  it("diffs an edit's old and new strings under its path", () => {
    render(
      <EditToolRenderer
        message={tool({
          name: "Edit",
          input: {
            file_path: "src/main.rs",
            old_string: "let a = 1;\nlet b = 2;\n",
            new_string: "let a = 1;\nlet b = 3;\n",
          },
          result: "The file src/main.rs has been updated.",
        })}
      />,
    );

    expect(screen.getByText("src/main.rs")).toBeDefined();
    expect(screen.getByText("let b = 2;")).toBeDefined();
    expect(screen.getByText("let b = 3;")).toBeDefined();
    expect(screen.getByText("+1")).toBeDefined();
    expect(screen.getByText("−1")).toBeDefined();
    expect(
      screen.getByText("The file src/main.rs has been updated."),
    ).toBeDefined();
  });

  it("offers side by side and comes back to unified", () => {
    render(
      <EditToolRenderer
        message={tool({
          name: "Edit",
          input: { file_path: "a.txt", old_string: "x\n", new_string: "y\n" },
        })}
      />,
    );

    const toggle = screen.getByRole("button", { name: "Side by side" });
    fireEvent.click(toggle);
    expect(screen.getByRole("button", { name: "Unified" })).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Unified" }));
    expect(screen.getByRole("button", { name: "Side by side" })).toBeDefined();
  });

  it("gives a multi-edit one diff per edit", () => {
    render(
      <EditToolRenderer
        message={tool({
          name: "MultiEdit",
          input: {
            file_path: "src/lib.ts",
            edits: [
              { old_string: "one", new_string: "uno" },
              { old_string: "two", new_string: "dos" },
            ],
          },
        })}
      />,
    );

    expect(screen.getByText("src/lib.ts · edit 1 of 2")).toBeDefined();
    expect(screen.getByText("src/lib.ts · edit 2 of 2")).toBeDefined();
    expect(screen.getByText("dos")).toBeDefined();
  });

  it("renders a write as pure additions", () => {
    render(
      <EditToolRenderer
        message={tool({
          name: "Write",
          input: { file_path: "new.txt", content: "first\nsecond\n" },
        })}
      />,
    );

    expect(screen.getByText("+2")).toBeDefined();
    expect(screen.getByText("−0")).toBeDefined();
    expect(screen.getByText("second")).toBeDefined();
  });

  it("names the cell a notebook edit rewrote", () => {
    render(
      <EditToolRenderer
        message={tool({
          name: "NotebookEdit",
          input: {
            notebook_path: "analysis.ipynb",
            new_source: "import pandas",
            cell_id: "cell-7",
          },
        })}
      />,
    );

    expect(screen.getByText("analysis.ipynb · cell cell-7")).toBeDefined();
    expect(screen.getByText("import pandas")).toBeDefined();
  });

  it("falls back to the JSON tree on an unexpected shape", () => {
    render(
      <EditToolRenderer
        message={tool({ name: "Edit", input: { path: "no old_string here" } })}
      />,
    );

    expect(screen.getByText("input:")).toBeDefined();
    expect(screen.getByText('"no old_string here"')).toBeDefined();
  });
});

describe("ShellToolRenderer", () => {
  it("shows the command, its description and ANSI-free output", () => {
    render(
      <ShellToolRenderer
        message={tool({
          name: "Bash",
          input: { command: "cargo test", description: "Run the test suite" },
          result: "\u001B[32mok\u001B[0m\r\n1 passed\n",
        })}
      />,
    );

    expect(screen.getByText("cargo test")).toBeDefined();
    expect(screen.getByText("Run the test suite")).toBeDefined();
    expect(screen.getByText(/ok 1 passed/)).toBeDefined();
    expect(document.body.textContent).not.toContain("\u001B");
  });

  it("collapses output above forty lines and expands it again", () => {
    const lines = Array.from({ length: 60 }, (_, i) => `line ${i + 1}`);
    render(
      <ShellToolRenderer
        message={tool({
          name: "Bash",
          input: { command: "seq 60" },
          result: lines.join("\n"),
        })}
      />,
    );

    expect(screen.queryByText(/line 60/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Show all 60 lines/ }));
    expect(screen.getByText(/line 60/)).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: /Collapse/ }));
    expect(screen.queryByText(/line 60/)).toBeNull();
  });
});

describe("SummaryToolRenderer", () => {
  it("summarises a read in one line over its result and folds it away", () => {
    render(
      <SummaryToolRenderer
        message={tool({
          name: "Read",
          input: { file_path: "README.md", offset: 10, limit: 20 },
          result: "# Mars\n",
        })}
      />,
    );

    expect(screen.getByText("Read README.md :10-20")).toBeDefined();
    expect(screen.getByText("# Mars")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: /Read README.md/ }));
    expect(screen.queryByText("# Mars")).toBeNull();
  });

  it("summarises the search and web tools", () => {
    const { rerender } = render(
      <SummaryToolRenderer
        message={tool({
          name: "Grep",
          input: { pattern: "TODO", path: "src" },
        })}
      />,
    );
    expect(screen.getByText("Grep TODO in src")).toBeDefined();

    rerender(
      <SummaryToolRenderer
        message={tool({ name: "Glob", input: { pattern: "**/*.rs" } })}
      />,
    );
    expect(screen.getByText("Glob **/*.rs")).toBeDefined();

    rerender(
      <SummaryToolRenderer
        message={tool({
          name: "WebSearch",
          input: { query: "axum websocket" },
        })}
      />,
    );
    expect(screen.getByText("WebSearch axum websocket")).toBeDefined();
  });
});

describe("JsonToolRenderer", () => {
  it("draws input and a structured result as trees", () => {
    render(
      <JsonToolRenderer
        message={tool({
          name: "mcp__bears__create_task",
          input: { title: "Do the thing", priority: "P1" },
          result: { id: "zxxj2", ok: true },
        })}
      />,
    );

    expect(screen.getByText('"Do the thing"')).toBeDefined();
    expect(screen.getByText('"zxxj2"')).toBeDefined();
    expect(screen.getByText("true")).toBeDefined();
  });

  it("joins text content blocks and leaves other blocks as a tree", () => {
    const { rerender } = render(
      <JsonToolRenderer
        message={tool({
          name: "mcp__x__y",
          result: [
            { type: "text", text: "one" },
            { type: "text", text: "two" },
          ],
        })}
      />,
    );
    expect(screen.getByText("one two")).toBeDefined();

    rerender(
      <JsonToolRenderer
        message={tool({
          name: "mcp__x__y",
          result: [{ type: "image", source: "…" }],
        })}
      />,
    );
    expect(screen.getByText("result:")).toBeDefined();
  });

  it("folds a node below depth two and opens it on click", () => {
    render(
      <JsonToolRenderer
        message={tool({
          name: "mcp__x__y",
          input: { a: { b: { c: "deep" } } },
        })}
      />,
    );

    expect(screen.queryByText('"deep"')).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /b:/ }));
    expect(screen.getByText('"deep"')).toBeDefined();
  });

  it("truncates a long string until it is expanded", () => {
    const long = "x".repeat(600);
    render(
      <JsonToolRenderer
        message={tool({ name: "mcp__x__y", input: { blob: long } })}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "100 more characters" }));
    expect(screen.getByText(`"${long}"`)).toBeDefined();
  });
});

describe("ToolFrame", () => {
  it("starts folded with the call summarised beside the tool's name", () => {
    render(
      <ToolFrame
        message={tool({
          name: "Bash",
          input: { command: "cargo test", description: "Run the tests" },
          result: "ok",
        })}
      />,
    );

    expect(screen.getByRole("button", { expanded: false })).toBeDefined();
    expect(screen.getByText("Run the tests")).toBeDefined();
    expect(screen.queryByText("ok")).toBeNull();
  });

  it("picks the family renderer for the tool's name once opened", () => {
    render(
      <ToolFrame
        message={tool({
          name: "Read",
          input: { file_path: "a.rs" },
          result: "fn main",
        })}
      />,
    );

    expect(screen.getByText("a.rs")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: /Read/ }));
    // One click reaches the result: the summary renderer starts open.
    expect(screen.getByText("Read a.rs")).toBeDefined();
    expect(screen.getByText("fn main")).toBeDefined();
  });

  it("says when the orchestrator cut the result", () => {
    render(
      <ToolFrame
        defaultOpen
        message={tool({
          name: "Bash",
          input: { command: "cat big" },
          result: "head",
          truncated: true,
        })}
      />,
    );

    expect(
      screen.getByText(
        "Result truncated at 256 KiB; the full output is in the session transcript file",
      ),
    ).toBeDefined();
  });

  it("draws a subagent's group once, not once more in the body", () => {
    const message = tool({
      name: "Task",
      input: { description: "Explore", prompt: "look around" },
      subagent: { description: "look at the repo", agent_type: "Explore" },
    });

    render(
      <ToolFrame message={message}>
        <SubagentGroup message={message}>
          <div>nested row</div>
        </SubagentGroup>
      </ToolFrame>,
    );

    expect(screen.getAllByText("Explore")).toHaveLength(1);
    // Opening the frame shows the call that started the subagent, not a
    // second copy of the group.
    fireEvent.click(screen.getByRole("button", { name: /Task/ }));
    expect(screen.getAllByText("Explore")).toHaveLength(1);
    expect(screen.getByText('"look around"')).toBeDefined();
  });
});
