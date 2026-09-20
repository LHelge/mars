// Which renderer draws the body of a tool message.
//
// `SPEC.md`, "Transcript rendering", picks one renderer per tool family by tool
// name. This module is the seam the families plug into, so `ToolFrame` never
// grows a switch over tool names; `renderers.ts` holds the registrations and
// `ToolFrame` imports it, so a lookup always finds a populated registry.
//
// A module that renders a component may export nothing else, so the lookup
// lives here and every component lives in its own `.tsx`.

import type { ComponentType } from "react";

import type { ToolMessage } from "../sessionStore";
import { JsonToolRenderer } from "./JsonToolRenderer";

export type ToolRenderer = ComponentType<{ message: ToolMessage }>;

interface Entry {
  matcher: (name: string) => boolean;
  component: ToolRenderer;
}

const entries: Entry[] = [];

/**
 * Registers a renderer for every tool name `matcher` accepts. Registration
 * order is resolution order: the first match wins, so a later registration
 * never shadows an earlier one.
 */
export function registerToolRenderer(
  matcher: (name: string) => boolean,
  component: ToolRenderer,
): void {
  entries.push({ matcher, component });
}

/**
 * The first registered renderer that claims `name`. Everything else — MCP
 * tools, tools a newer CLI added, the `unknown` of a result without a call —
 * is drawn as a JSON tree.
 */
export function toolRendererFor(name: string): ToolRenderer {
  return (
    entries.find((entry) => entry.matcher(name))?.component ?? JsonToolRenderer
  );
}

/**
 * A matcher for a fixed set of tool names: exact first, then case-insensitive,
 * because the same family is spelled `Bash` by one CLI version and `bash` by
 * the next.
 */
export function byName(...names: string[]): (name: string) => boolean {
  const folded = new Set(names.map((one) => one.toLowerCase()));
  const exact = new Set(names);
  return (name: string) => exact.has(name) || folded.has(name.toLowerCase());
}

/** Test-only: drops every registration made so far. */
export function clearToolRenderers(): void {
  entries.length = 0;
}
