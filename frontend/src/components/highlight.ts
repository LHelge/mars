// The syntax highlighter, and nothing else: this module is the lazy chunk
// (`SPEC.md`, "Frontend", "Code splitting"). Nothing imports it statically —
// `CodeBlock` reaches it through `import("./highlight")` once a fence with a
// language has settled — so `lowlight` and the language definitions below are
// fetched the first time a reader is actually shown code, and never on a first
// paint.
//
// `lowlight` over `highlight.js` directly, and over `rehype-highlight` in the
// markdown pipeline: it returns a hast tree rather than an HTML string, so the
// tokens become React elements here without `dangerouslySetInnerHTML`, which
// the markdown renderer refuses for the same reason it has no `rehype-raw`
// (ADR 0040). Keeping it out of the `react-markdown` plugin list is what lets
// it be lazy at all.
//
// The language set is explicit, because every entry is bytes in this chunk,
// and there is no auto-detection: a fence with no language stays plain text,
// which costs nothing and is never wrong. Most of the aliases a writer reaches
// for — `rs`, `ts`, `tsx`, `js`, `sh`, `zsh`, `console`, `yml`, `md`, `toml`,
// `docker`, `html`, `patch` — are declared by the language definitions
// themselves and registered with them; `ALIASES` below carries only the few
// tags `highlight.js` does not know.

import type { Element, RootContent } from "hast";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
// `ini` is the definition that carries the `toml` alias.
import ini from "highlight.js/lib/languages/ini";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import shell from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
// `xml` is the definition that carries `html`, `svg` and friends.
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { createLowlight } from "lowlight";
import type { ReactNode } from "react";
import { createElement } from "react";

const lowlight = createLowlight({
  bash,
  css,
  diff,
  dockerfile,
  go,
  ini,
  javascript,
  json,
  markdown,
  python,
  rust,
  shell,
  sql,
  typescript,
  xml,
  yaml,
});

/** Tags in the wild that `highlight.js` does not declare itself. */
const ALIASES: Record<string, string> = {
  jsonc: "json",
  json5: "json",
  "shell-session": "shell",
  "bash-session": "shell",
  tf: "ini",
  conf: "ini",
};

function normalise(language: string): string {
  const tag = language.trim().toLowerCase();
  return ALIASES[tag] ?? tag;
}

// hast to React, by hand: `lowlight` emits only `span` elements carrying a
// `className` array and text nodes, so the whole tree is those two cases.
function toNodes(children: RootContent[], prefix: string): ReactNode[] {
  return children.map((child, index) => {
    const key = `${prefix}-${String(index)}`;
    if (child.type === "text") return child.value;
    if (child.type !== "element") return null;
    const element: Element = child;
    const classes = element.properties.className;
    return createElement(
      "span",
      {
        key,
        className: Array.isArray(classes) ? classes.join(" ") : undefined,
      },
      ...toNodes(element.children, key),
    );
  });
}

/**
 * The fence's code as coloured spans, or `null` when the tag names no language
 * this chunk knows — in which case the caller leaves the block plain.
 */
export function highlightCode(
  language: string,
  code: string,
): ReactNode[] | null {
  const name = normalise(language);
  if (!lowlight.registered(name)) return null;
  return toNodes(lowlight.highlight(name, code).children, "hl");
}
