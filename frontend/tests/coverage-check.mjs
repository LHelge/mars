#!/usr/bin/env node
// Checks the coverage table in `frontend/tests/README.md` against the suite it
// describes, so the table cannot quietly go stale.
//
// Two checks, and `npm run test:e2e` runs both:
//
//   node tests/coverage-check.mjs            the table (a `pretest:e2e` step)
//   node tests/coverage-check.mjs --skips    the last run (a `posttest:e2e` step)
//
// The first reads every `` `<file>.spec.ts` › `<title>` `` reference out of the
// table and fails on one that names a spec file that does not exist, or a test
// title that file does not contain. It also fails on a `test.skip` in the suite
// that the table does not list as an exclusion: a scenario that stops running
// has to say so in the document, not only in the code.
//
// The second reads Playwright's JSON report and fails on a skipped test that is
// not one of the two documented skips. That is the check the seeded-admin
// scenario needs: with `retries: 1` a failure that was retried into a skip would
// otherwise be reported as a pass.

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const TESTS_DIR = dirname(fileURLToPath(import.meta.url));
const README = join(TESTS_DIR, "README.md");
const REPORT = resolve(TESTS_DIR, "..", "test-results", "results.json");

/**
 * The skips the table documents, by title. A skip that is not here fails the
 * check whether it comes from the source or from a run.
 */
const DOCUMENTED_SKIPS = new Set([
  "the github compare link is built client-side",
  "must change password before anything else",
]);

/** `` `<file>.spec.ts` › `<test title>` ``, the table's one reference form. */
const REFERENCE = /`([a-z-]+\.spec\.ts)`\s*›\s*`([^`]+)`/g;

/** `test("…"` and `test.skip("…"`, however the file wraps the call. */
const TEST_TITLE = /\btest(\.skip|\.fixme)?\(\s*\n?\s*"((?:[^"\\]|\\.)*)"/g;

function fail(problems) {
  console.error("tests/README.md coverage check failed:");
  for (const problem of problems) console.error(`  - ${problem}`);
  process.exit(1);
}

/** Every test title in a spec file, and the ones declared `test.skip`. */
function titlesOf(file) {
  const source = readFileSync(join(TESTS_DIR, file), "utf8");
  const all = new Set();
  const skipped = new Set();
  for (const match of source.matchAll(TEST_TITLE)) {
    const title = match[2].replace(/\\(.)/g, "$1");
    all.add(title);
    if (match[1] !== undefined) skipped.add(title);
  }
  return { all, skipped };
}

function checkTable() {
  const readme = readFileSync(README, "utf8");
  const specs = readdirSync(TESTS_DIR).filter((name) =>
    name.endsWith(".spec.ts"),
  );
  const problems = [];
  const referenced = new Map();

  for (const [, file, title] of readme.matchAll(REFERENCE)) {
    if (!specs.includes(file)) {
      problems.push(`the table names ${file}, which is not a spec file`);
      continue;
    }
    if (!referenced.has(file)) referenced.set(file, titlesOf(file));
    if (!referenced.get(file).all.has(title)) {
      problems.push(`${file} has no test titled "${title}"`);
    }
  }

  if (referenced.size === 0) {
    problems.push("the table references no scenario at all: has its shape changed?");
  }

  // A skipped scenario is an exclusion, and an exclusion belongs in the table.
  for (const file of specs) {
    for (const title of titlesOf(file).skipped) {
      if (!DOCUMENTED_SKIPS.has(title)) {
        problems.push(
          `${file} skips "${title}", which is not a documented exclusion ` +
            "(add it to the table and to DOCUMENTED_SKIPS)",
        );
      }
    }
  }

  if (problems.length > 0) fail(problems);
  console.log(
    `tests/README.md: ${String(referenced.size)} spec files referenced, every title found`,
  );
}

function checkSkips() {
  let report;
  try {
    report = JSON.parse(readFileSync(REPORT, "utf8"));
  } catch {
    console.log(`no Playwright report at ${REPORT}; skip check not run`);
    return;
  }

  const problems = [];
  const walk = (suite) => {
    for (const spec of suite.specs ?? []) {
      for (const test of spec.tests ?? []) {
        const status = test.status ?? "";
        if (status !== "skipped") continue;
        if (DOCUMENTED_SKIPS.has(spec.title)) continue;
        problems.push(`"${spec.title}" was skipped and is not a documented skip`);
      }
    }
    for (const child of suite.suites ?? []) walk(child);
  };
  for (const suite of report.suites ?? []) walk(suite);

  if (problems.length > 0) fail(problems);
  console.log("the run skipped nothing the table does not document");
}

if (process.argv.includes("--skips")) {
  checkSkips();
} else {
  checkTable();
}
