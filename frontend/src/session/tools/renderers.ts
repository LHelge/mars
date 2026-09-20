// The tool families of `SPEC.md`, "Transcript rendering", and the names that
// belong to each. Importing this module registers them; `ToolFrame` imports it
// so the registry is populated wherever a tool row is drawn.
//
// Registration order is resolution order, so the families are listed from the
// most specific outwards. There is no catch-all entry: `toolRendererFor` falls
// back to the JSON tree on its own.

import { SubagentGroup } from "../SubagentGroup";
import { EditToolRenderer } from "./EditToolRenderer";
import { JsonToolRenderer } from "./JsonToolRenderer";
import { byName, registerToolRenderer } from "./registry";
import { ShellToolRenderer } from "./ShellToolRenderer";
import { SummaryToolRenderer } from "./SummaryToolRenderer";

/**
 * Registers every built-in family, in resolution order. Called once at module
 * load; a test that cleared the registry calls it again to restore it, so no
 * test depends on which file imported this module first.
 */
export function registerBuiltinToolRenderers(): void {
  registerToolRenderer(
    byName("Edit", "MultiEdit", "Write", "NotebookEdit"),
    EditToolRenderer,
  );
  registerToolRenderer(byName("Bash"), ShellToolRenderer);
  registerToolRenderer(
    byName("Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch"),
    SummaryToolRenderer,
  );
  // The subagent tool is `Task` on one CLI version and `Agent` on the next
  // (`SPEC.md`, "AgentEvent" translation rules); both are the same family.
  // `MessageRow` draws the nested transcript as the frame's children, so
  // `ToolFrame` resolves this entry but never instantiates it a second time.
  registerToolRenderer(byName("Task", "Agent"), SubagentGroup);
  registerToolRenderer(byName("unknown"), JsonToolRenderer);
}

registerBuiltinToolRenderers();
