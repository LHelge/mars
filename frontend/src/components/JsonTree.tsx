// The fallback view for structured values: tool input nothing more specific
// claims, tool results that are not text, and `raw` messages (`SPEC.md`,
// "Transcript rendering"). No dependency; the tree is a few rows of spans.
//
// Depth 0 and 1 are open, everything below starts folded: an MCP payload is
// usually one or two levels of envelope around the part worth reading.

import { useState } from "react";

/** Depth beyond which nodes start collapsed. */
const OPEN_DEPTH = 2;
/** Strings longer than this are cut, with the rest one click away. */
const STRING_LIMIT = 500;

type Json = unknown;

function isRecord(value: Json): value is Record<string, Json> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function StringValue({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false);
  const long = text.length > STRING_LIMIT;
  if (!long || expanded) {
    return <span className="text-state-running break-all">"{text}"</span>;
  }
  return (
    <span className="break-all">
      <span className="text-state-running">"{text.slice(0, STRING_LIMIT)}…"</span>{" "}
      <button
        type="button"
        onClick={() => setExpanded(true)}
        className="text-console-accent underline underline-offset-2"
      >
        {text.length - STRING_LIMIT} more characters
      </button>
    </span>
  );
}

function Scalar({ value }: { value: Json }) {
  if (typeof value === "string") {
    return <StringValue text={value} />;
  }
  if (value === null) {
    return <span className="text-console-muted">null</span>;
  }
  if (value === undefined) {
    return <span className="text-console-muted">undefined</span>;
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return <span className="text-state-parked">{String(value)}</span>;
  }
  // A bigint, symbol or function: nothing `JSON.parse` produces, so the type
  // name is more use than whatever `String` would make of it.
  return <span className="text-console-muted">{typeof value}</span>;
}

/** `{3 keys}` / `[5 items]`: what a folded node hides, without opening it. */
function summarise(value: Json): string {
  if (Array.isArray(value)) {
    return value.length === 1 ? "[1 item]" : `[${value.length} items]`;
  }
  if (isRecord(value)) {
    const keys = Object.keys(value);
    return keys.length === 1 ? "{1 key}" : `{${keys.length} keys}`;
  }
  return "";
}

function Branch({
  name,
  value,
  depth,
}: {
  name: string | null;
  value: Json;
  depth: number;
}) {
  const [open, setOpen] = useState(depth < OPEN_DEPTH);
  const entries: [string, Json][] = Array.isArray(value)
    ? value.map((item, index) => [String(index), item])
    : Object.entries(value as Record<string, Json>);

  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen((current) => !current)}
        aria-expanded={open}
        className="flex items-start gap-1 text-left"
      >
        <span aria-hidden="true" className="text-console-muted">
          {open ? "▾" : "▸"}
        </span>
        {name !== null && <span className="text-console-text">{name}:</span>}
        <span className="text-console-muted">{summarise(value)}</span>
      </button>
      {open && (
        <div className="border-console-border ml-1.5 border-l pl-3">
          {entries.length === 0 ? (
            <span className="text-console-muted">empty</span>
          ) : (
            entries.map(([key, child]) => (
              <Node key={key} name={key} value={child} depth={depth + 1} />
            ))
          )}
        </div>
      )}
    </div>
  );
}

function Node({
  name,
  value,
  depth,
}: {
  name: string | null;
  value: Json;
  depth: number;
}) {
  if (Array.isArray(value) || isRecord(value)) {
    return <Branch name={name} value={value} depth={depth} />;
  }
  return (
    <div className="flex items-start gap-1">
      {name !== null && (
        <span className="text-console-text shrink-0">{name}:</span>
      )}
      <Scalar value={value} />
    </div>
  );
}

export interface JsonTreeProps {
  value: unknown;
  /** The name of the root node; omitted for an anonymous value. */
  name?: string;
}

export function JsonTree({ value, name }: JsonTreeProps) {
  return (
    <div className="border-console-border bg-console-bg overflow-x-auto rounded border px-3 py-2 font-mono text-xs">
      <Node name={name ?? null} value={value} depth={0} />
    </div>
  );
}
