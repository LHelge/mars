import { describe, expect, it } from "vitest";

import {
  DIFF_LINE_CAP,
  lineDiff,
  lineDiffCapped,
  parseUnifiedPatch,
} from "./diff";

describe("lineDiff", () => {
  it("marks every line of an identical pair as context", () => {
    const lines = lineDiff("a\nb\nc\n", "a\nb\nc\n");
    expect(lines.map((line) => line.type)).toEqual([
      "context",
      "context",
      "context",
    ]);
    expect(lines[2]).toEqual({
      type: "context",
      text: "c",
      oldNo: 3,
      newNo: 3,
    });
  });

  it("aligns an insertion in the middle", () => {
    const lines = lineDiff("a\nc\n", "a\nb\nc\n");
    expect(lines).toEqual([
      { type: "context", text: "a", oldNo: 1, newNo: 1 },
      { type: "add", text: "b", newNo: 2 },
      { type: "context", text: "c", oldNo: 2, newNo: 3 },
    ]);
  });

  it("aligns a deletion in the middle", () => {
    const lines = lineDiff("a\nb\nc\n", "a\nc\n");
    expect(lines).toEqual([
      { type: "context", text: "a", oldNo: 1, newNo: 1 },
      { type: "del", text: "b", oldNo: 2 },
      { type: "context", text: "c", oldNo: 3, newNo: 2 },
    ]);
  });

  it("renders a replacement as a deletion followed by an addition", () => {
    const lines = lineDiff("a\nold\nc\n", "a\nnew\nc\n");
    expect(lines).toEqual([
      { type: "context", text: "a", oldNo: 1, newNo: 1 },
      { type: "del", text: "old", oldNo: 2 },
      { type: "add", text: "new", newNo: 2 },
      { type: "context", text: "c", oldNo: 3, newNo: 3 },
    ]);
  });

  it("renders an empty old text as a pure insertion", () => {
    expect(lineDiff("", "one\ntwo\n")).toEqual([
      { type: "add", text: "one", newNo: 1 },
      { type: "add", text: "two", newNo: 2 },
    ]);
    expect(lineDiff("", "")).toEqual([]);
  });

  it("numbers both sides independently once they drift apart", () => {
    const lines = lineDiff("keep\ndrop\ndrop2\ntail\n", "keep\nadd\ntail\n");
    expect(lines).toEqual([
      { type: "context", text: "keep", oldNo: 1, newNo: 1 },
      { type: "del", text: "drop", oldNo: 2 },
      { type: "del", text: "drop2", oldNo: 3 },
      { type: "add", text: "add", newNo: 2 },
      { type: "context", text: "tail", oldNo: 4, newNo: 3 },
    ]);
  });

  it("falls back to unaligned blocks above the line cap", () => {
    const big = `${Array.from({ length: DIFF_LINE_CAP + 1 }, (_, i) => `l${i}`).join("\n")}\n`;
    expect(lineDiffCapped(big, "one\n")).toBe(true);
    expect(lineDiffCapped("a\n", "b\n")).toBe(false);

    const lines = lineDiff(big, "one\n");
    expect(lines).toHaveLength(DIFF_LINE_CAP + 2);
    expect(lines[0]).toEqual({ type: "del", text: "l0", oldNo: 1 });
    expect(lines[lines.length - 1]).toEqual({
      type: "add",
      text: "one",
      newNo: 1,
    });
  });
});

const TWO_FILE_PATCH = `diff --git a/src/one.ts b/src/one.ts
index 1111111..2222222 100644
--- a/src/one.ts
+++ b/src/one.ts
@@ -1,3 +1,3 @@ export function one()
 const a = 1;
-const b = 2;
+const b = 3;
 const c = 4;
@@ -10,2 +10,3 @@
 tail
+extra
\\ No newline at end of file
diff --git a/src/two.ts b/src/two.ts
new file mode 100644
--- /dev/null
+++ b/src/two.ts
@@ -0,0 +1,2 @@
+first
+second
`;

describe("parseUnifiedPatch", () => {
  it("splits a two-file patch into files and hunks", () => {
    const files = parseUnifiedPatch(TWO_FILE_PATCH);
    expect(files.map((file) => file.path)).toEqual([
      "src/one.ts",
      "src/two.ts",
    ]);
    expect(files[0].hunks).toHaveLength(2);
    expect(files[1].hunks).toHaveLength(1);
  });

  it("keeps the hunk header and numbers the lines from it", () => {
    const [first] = parseUnifiedPatch(TWO_FILE_PATCH);
    expect(first.hunks[0].header).toBe(
      "@@ -1,3 +1,3 @@ export function one()",
    );
    expect(first.hunks[0].lines).toEqual([
      { type: "context", text: "const a = 1;", oldNo: 1, newNo: 1 },
      { type: "del", text: "const b = 2;", oldNo: 2 },
      { type: "add", text: "const b = 3;", newNo: 2 },
      { type: "context", text: "const c = 4;", oldNo: 3, newNo: 3 },
    ]);
    expect(first.hunks[1].lines).toEqual([
      { type: "context", text: "tail", oldNo: 10, newNo: 10 },
      { type: "add", text: "extra", newNo: 11 },
    ]);
  });

  it("reads a new file whose old side is /dev/null", () => {
    const files = parseUnifiedPatch(TWO_FILE_PATCH);
    expect(files[1].hunks[0].lines).toEqual([
      { type: "add", text: "first", newNo: 1 },
      { type: "add", text: "second", newNo: 2 },
    ]);
  });

  it("answers an empty list for a patch with no files", () => {
    expect(parseUnifiedPatch("")).toEqual([]);
  });

  it("marks a text file as not binary", () => {
    expect(parseUnifiedPatch(TWO_FILE_PATCH).map((file) => file.binary)).toEqual(
      [false, false],
    );
  });

  it("marks a file git refused to diff as binary, with no hunks", () => {
    const files = parseUnifiedPatch(
      `diff --git a/assets/logo.png b/assets/logo.png
index 1111111..2222222 100644
Binary files a/assets/logo.png and b/assets/logo.png differ
`,
    );
    expect(files).toEqual([
      { path: "assets/logo.png", hunks: [], binary: true },
    ]);
  });

  it("marks a --binary patch's literal payload as binary too", () => {
    const [file] = parseUnifiedPatch(
      `diff --git a/assets/logo.png b/assets/logo.png
new file mode 100644
index 0000000..2222222
GIT binary patch
literal 8
`,
    );
    expect(file.binary).toBe(true);
  });

  it("names a renamed file by its new path", () => {
    const files = parseUnifiedPatch(
      `diff --git a/src/old.ts b/src/new.ts
similarity index 95%
rename from src/old.ts
rename to src/new.ts
--- a/src/old.ts
+++ b/src/new.ts
@@ -1,1 +1,1 @@
-const a = 1;
+const a = 2;
`,
    );
    expect(files.map((file) => file.path)).toEqual(["src/new.ts"]);
    expect(files[0].hunks[0].lines).toEqual([
      { type: "del", text: "const a = 1;", oldNo: 1 },
      { type: "add", text: "const a = 2;", newNo: 1 },
    ]);
  });
});
