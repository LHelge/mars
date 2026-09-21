// The first-paint budget of `SPEC.md`, "Frontend", "Code splitting": the entry
// bundle carries only what a first paint needs.
//
// Why this is a build check and not a review habit: the leak it catches is
// invisible in the source. One `import { Alert } from "./components"` on the
// entry path pulled the whole UI kit — the secrets manager, the git panel, the
// diff and JSON views — into the graph Rollup must have ready before the first
// render. Rollup then splits it into a chunk of its own, so the entry chunk's
// own size never moved, and `index.html` quietly gained a `modulepreload` for
// 40 kB of feature UI nobody on the login page will ever see.
//
// So the unit checked here is not the entry chunk but the *first-paint set*:
// the entry script plus every module it statically imports, which is exactly
// what Vite emits `modulepreload` links for. A lazy route's chunk is fetched on
// navigation and is not in that set.
//
// Two assertions over that set:
//
//   * no marker string of a lazily routed feature appears in it, and
//   * it stays under a byte budget.
//
// Each marker is also asserted to exist *somewhere* in `dist`, so renaming a
// button does not silently retire the check that watches it.
//
// Run by `npm run build` (`README.md`, "Development"), so Frontend CI runs it
// too.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dist = resolve(dirname(fileURLToPath(import.meta.url)), "..", "dist");

/**
 * A string that only the feature behind a lazy route renders, and the route it
 * belongs to. Copy, not identifiers: a minifier renames identifiers and leaves
 * string literals alone.
 */
const MARKERS = [
  { route: "/secrets, /projects/:id (secrets)", needle: "Add secret" },
  { route: "/secrets, /projects/:id (secrets)", needle: "Replace value" },
  { route: "/secrets (secret scope)", needle: "orchestrator_only" },
  { route: "/sessions/:id, /projects/:id (git)", needle: "Force push" },
  { route: "/sessions/:id, /projects/:id (git)", needle: "Rebase" },
  { route: "/projects/:id (task board)", needle: "Move to" },
];

/**
 * Bytes of JavaScript a first paint must download, uncompressed. Headroom over
 * today's figure, which the build prints; a change that needs more than this is
 * a change to what a first paint is, and says so by moving the number.
 */
const BUDGET = 400_000;

function fail(lines) {
  console.error(`check-entry-chunk: ${lines.join("\n  ")}`);
  process.exit(1);
}

let html;
try {
  html = readFileSync(join(dist, "index.html"), "utf8");
} catch {
  fail(["dist/index.html is missing — run `vite build` first"]);
}

// The entry script and everything Vite preloads beside it: the static import
// closure of the entry, which is what the browser fetches before first paint.
const firstPaint = [
  ...html.matchAll(/<script[^>]+src="\/([^"]+\.js)"/g),
  ...html.matchAll(/<link[^>]+rel="modulepreload"[^>]+href="\/([^"]+\.js)"/g),
].map((m) => m[1]);

if (firstPaint.length === 0) {
  fail(["no entry script found in dist/index.html"]);
}

function walk(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

const allJs = walk(dist).filter((p) => p.endsWith(".js"));
const everything = allJs.map((p) => readFileSync(p, "utf8"));

const sources = new Map(
  firstPaint.map((rel) => [rel, readFileSync(join(dist, rel), "utf8")]),
);

const problems = [];

for (const { route, needle } of MARKERS) {
  if (!everything.some((text) => text.includes(needle))) {
    problems.push(
      `marker ${JSON.stringify(needle)} (${route}) is in no chunk at all — ` +
        `the copy changed, so update MARKERS rather than losing the check`,
    );
    continue;
  }
  for (const [rel, text] of sources) {
    if (text.includes(needle)) {
      problems.push(
        `${rel} is fetched before first paint and contains ` +
          `${JSON.stringify(needle)}, which belongs to ${route}: something on ` +
          `the entry path imports that feature, most likely through a barrel`,
      );
    }
  }
}

const bytes = [...sources.values()].reduce((sum, text) => sum + text.length, 0);
if (bytes > BUDGET) {
  problems.push(
    `first paint fetches ${String(bytes)} bytes of JavaScript, over the ` +
      `${String(BUDGET)} budget: ${firstPaint.join(", ")}`,
  );
}

if (problems.length > 0) {
  fail(problems);
}

console.log(
  `check-entry-chunk: first paint is ${String(firstPaint.length)} chunks, ` +
    `${String(bytes)} bytes (budget ${String(BUDGET)}), free of feature UI`,
);
