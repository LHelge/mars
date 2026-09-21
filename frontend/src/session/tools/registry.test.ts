// Which renderer each tool name resolves to (`SPEC.md`, "Transcript
// rendering"). The registry is cleared and repopulated here rather than
// trusted as imported, so the result does not depend on which test file ran
// first or on how many times the module graph was evaluated.

import { beforeEach, describe, expect, it } from "vitest";

import { EditToolRenderer } from "./EditToolRenderer";
import { JsonToolRenderer } from "./JsonToolRenderer";
import { ShellToolRenderer } from "./ShellToolRenderer";
import { SummaryToolRenderer } from "./SummaryToolRenderer";
import {
  byName,
  clearToolRenderers,
  registerToolRenderer,
  toolRendererFor,
} from "./registry";
import { registerBuiltinToolRenderers } from "./renderers";

beforeEach(() => {
  clearToolRenderers();
  registerBuiltinToolRenderers();
});

describe("toolRendererFor", () => {
  it("resolves the edit family", () => {
    for (const name of ["Edit", "MultiEdit", "Write", "NotebookEdit"]) {
      expect(toolRendererFor(name)).toBe(EditToolRenderer);
    }
  });

  it("resolves the shell family", () => {
    expect(toolRendererFor("Bash")).toBe(ShellToolRenderer);
  });

  it("resolves the read, search and web family", () => {
    for (const name of [
      "Read",
      "Glob",
      "Grep",
      "LS",
      "WebFetch",
      "WebSearch",
    ]) {
      expect(toolRendererFor(name)).toBe(SummaryToolRenderer);
    }
  });

  it("leaves the subagent family to the JSON tree", () => {
    // No family claims `Task` or `Agent`: a call a `subagent_start` has
    // announced is drawn by `ToolFrame` from the message itself, and until
    // that event arrives the input and the result are all there is to show.
    expect(toolRendererFor("Task")).toBe(JsonToolRenderer);
    expect(toolRendererFor("Agent")).toBe(JsonToolRenderer);
  });

  it("falls back to the JSON tree for anything else", () => {
    expect(toolRendererFor("mcp__bears__create_task")).toBe(JsonToolRenderer);
    expect(toolRendererFor("unknown")).toBe(JsonToolRenderer);
    expect(toolRendererFor("SomeToolFromANewerCli")).toBe(JsonToolRenderer);
  });

  it("matches a name whose case differs", () => {
    expect(toolRendererFor("bash")).toBe(ShellToolRenderer);
    expect(toolRendererFor("multiedit")).toBe(EditToolRenderer);
    expect(toolRendererFor("websearch")).toBe(SummaryToolRenderer);
  });

  it("keeps the first registration when two claim a name", () => {
    clearToolRenderers();
    registerToolRenderer(byName("Bash"), JsonToolRenderer);
    registerBuiltinToolRenderers();
    expect(toolRendererFor("Bash")).toBe(JsonToolRenderer);
  });
});
