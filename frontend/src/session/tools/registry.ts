// Which renderer draws the body of a tool message.
//
// `SPEC.md`, "Transcript rendering", picks one renderer per tool family by tool
// name. The families themselves are the next task; this module is the seam they
// plug into, so `ToolFrame` never grows a switch over tool names.
//
// A module that renders a component may export nothing else, so the lookup
// lives here and every component lives in its own `.tsx`.

import type { ComponentType } from "react";

import type { ToolMessage } from "../sessionStore";
import { DefaultToolRenderer } from "./DefaultToolRenderer";

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

/** The first registered renderer that claims `name`, else the default. */
export function toolRendererFor(name: string): ToolRenderer {
  return (
    entries.find((entry) => entry.matcher(name))?.component ??
    DefaultToolRenderer
  );
}

/** Test-only: drops every registration made so far. */
export function clearToolRenderers(): void {
  entries.length = 0;
}
