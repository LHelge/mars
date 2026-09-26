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
    expect(files.map((file) => file?.path)).toEqual([
      "src/one.ts",
      "src/two.ts",
    ]);
    expect(files[0]?.hunks).toHaveLength(2);
    expect(files[1]?.hunks).toHaveLength(1);
  });

  it("keeps the hunk header and numbers the lines from it", () => {
    const [first] = parseUnifiedPatch(TWO_FILE_PATCH);
    expect(first?.hunks[0]?.header).toBe(
      "@@ -1,3 +1,3 @@ export function one()",
    );
    expect(first?.hunks[0]?.lines).toEqual([
      { type: "context", text: "const a = 1;", oldNo: 1, newNo: 1 },
      { type: "del", text: "const b = 2;", oldNo: 2 },
      { type: "add", text: "const b = 3;", newNo: 2 },
      { type: "context", text: "const c = 4;", oldNo: 3, newNo: 3 },
    ]);
    expect(first?.hunks[1]?.lines).toEqual([
      { type: "context", text: "tail", oldNo: 10, newNo: 10 },
      { type: "add", text: "extra", newNo: 11 },
    ]);
  });

  it("reads a new file whose old side is /dev/null", () => {
    const files = parseUnifiedPatch(TWO_FILE_PATCH);
    expect(files[1]?.hunks[0]?.lines).toEqual([
      { type: "add", text: "first", newNo: 1 },
      { type: "add", text: "second", newNo: 2 },
    ]);
  });

  it("answers an empty list for a patch with no files", () => {
    expect(parseUnifiedPatch("")).toEqual([]);
  });

  it("marks a text file as not binary", () => {
    expect(
      parseUnifiedPatch(TWO_FILE_PATCH).map((file) => file?.binary),
    ).toEqual([false, false]);
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
    expect(file?.binary).toBe(true);
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
    expect(files.map((file) => file?.path)).toEqual(["src/new.ts"]);
    expect(files[0]?.hunks[0]?.lines).toEqual([
      { type: "del", text: "const a = 1;", oldNo: 1 },
      { type: "add", text: "const a = 2;", newNo: 1 },
    ]);
  });

  it("keeps a deleted `-- ` comment as a deletion and keeps counting", () => {
    // What deleting a line of SQL, Lua or Haskell looks like: the `-` marker
    // in front of `-- old comment` spells a file header.
    const [file] = parseUnifiedPatch(
      `diff --git a/migrations/0001_init.sql b/migrations/0001_init.sql
index 1111111..2222222 100644
--- a/migrations/0001_init.sql
+++ b/migrations/0001_init.sql
@@ -1,3 +1,3 @@
 CREATE TABLE t (id uuid);
--- old comment
+-- new comment
 SELECT 1;
`,
    );
    expect(file?.path).toBe("migrations/0001_init.sql");
    expect(file?.hunks[0]?.lines).toEqual([
      {
        type: "context",
        text: "CREATE TABLE t (id uuid);",
        oldNo: 1,
        newNo: 1,
      },
      { type: "del", text: "-- old comment", oldNo: 2 },
      { type: "add", text: "-- new comment", newNo: 2 },
      // The old side kept counting: the deletion was not swallowed.
      { type: "context", text: "SELECT 1;", oldNo: 3, newNo: 3 },
    ]);
  });

  it("keeps an added `++ ` line as an addition and does not rename the file", () => {
    const [file] = parseUnifiedPatch(
      `diff --git a/notes.txt b/notes.txt
index 1111111..2222222 100644
--- a/notes.txt
+++ b/notes.txt
@@ -1,1 +1,2 @@
 a
+++ b
`,
    );
    expect(file?.path).toBe("notes.txt");
    expect(file?.hunks[0]?.lines).toEqual([
      { type: "context", text: "a", oldNo: 1, newNo: 1 },
      { type: "add", text: "++ b", newNo: 2 },
    ]);
  });

  it("does not count a no-newline marker as a line of the hunk", () => {
    const [file] = parseUnifiedPatch(
      `diff --git a/tail.txt b/tail.txt
index 1111111..2222222 100644
--- a/tail.txt
+++ b/tail.txt
@@ -1,1 +1,1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
diff --git a/after.txt b/after.txt
new file mode 100644
`,
    );
    expect(file?.hunks[0]?.lines).toEqual([
      { type: "del", text: "old", oldNo: 1 },
      { type: "add", text: "new", newNo: 1 },
    ]);
  });

  it("reads a hunk header that omits its counts as one line a side", () => {
    const [file] = parseUnifiedPatch(
      `diff --git a/one.txt b/one.txt
--- a/one.txt
+++ b/one.txt
@@ -3 +3 @@
-old
+new
`,
    );
    expect(file?.hunks[0]?.lines).toEqual([
      { type: "del", text: "old", oldNo: 3 },
      { type: "add", text: "new", newNo: 3 },
    ]);
  });

  it("lists a new empty file and a mode-only change with no hunks", () => {
    expect(
      parseUnifiedPatch(
        `diff --git a/empty.txt b/empty.txt
new file mode 100644
index 0000000..e69de29
diff --git a/script.sh b/script.sh
old mode 100644
new mode 100755
`,
      ),
    ).toEqual([
      { path: "empty.txt", hunks: [], binary: false },
      { path: "script.sh", hunks: [], binary: false },
    ]);
  });

  it("keeps the carriage returns of a CRLF file and still reads the headers", () => {
    const [file] = parseUnifiedPatch(
      "diff --git a/crlf.txt b/crlf.txt\r\n" +
        "index 1111111..2222222 100644\r\n" +
        "--- a/crlf.txt\r\n" +
        "+++ b/crlf.txt\r\n" +
        "@@ -1,2 +1,2 @@\r\n" +
        " one\r\n" +
        "-two\r\n" +
        "+three\r\n",
    );
    expect(file?.path).toBe("crlf.txt");
    expect(file?.hunks[0]?.header).toBe("@@ -1,2 +1,2 @@");
    expect(file?.hunks[0]?.lines).toEqual([
      { type: "context", text: "one\r", oldNo: 1, newNo: 1 },
      { type: "del", text: "two\r", oldNo: 2 },
      { type: "add", text: "three\r", newNo: 2 },
    ]);
  });

  it("unquotes a C-quoted path", () => {
    const [file] = parseUnifiedPatch(
      `diff --git "a/docs/f\\303\\266o.md" "b/docs/f\\303\\266o.md"
index 1111111..2222222 100644
--- "a/docs/f\\303\\266o.md"
+++ "b/docs/f\\303\\266o.md"
@@ -1 +1 @@
-old
+new
`,
    );
    expect(file?.path).toBe("docs/föo.md");
    expect(file?.hunks[0]?.lines).toHaveLength(2);
  });

  it("marks the binary file after a text file, not the text file", () => {
    const files = parseUnifiedPatch(
      `diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-one
+two
diff --git "a/img/f\\303\\266o.png" "b/img/f\\303\\266o.png"
new file mode 100644
index 0000000..3333333
Binary files /dev/null and "b/img/f\\303\\266o.png" differ
`,
    );
    expect(files.map((file) => [file?.path, file?.binary])).toEqual([
      ["a.txt", false],
      ["img/föo.png", true],
    ]);
    expect(files[0]?.hunks).toHaveLength(1);
    expect(files[1]?.hunks).toEqual([]);
  });

  it("starts a new file on a `diff --git` line whose paths it cannot read", () => {
    const files = parseUnifiedPatch(
      `diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-one
+two
diff --git nonsense
Binary files a/nonsense and b/nonsense differ
`,
    );
    expect(files).toHaveLength(2);
    expect(files[0]?.binary).toBe(false);
    expect(files[1]?.binary).toBe(true);
  });

  it("splits a plain `diff -u` patch that has no `diff --git` lines", () => {
    const files = parseUnifiedPatch(
      `--- a/one.txt
+++ b/one.txt
@@ -1 +1 @@
-one
+ONE
--- a/two.txt
+++ b/two.txt
@@ -1 +1 @@
-two
+TWO
`,
    );
    expect(files.map((file) => file?.path)).toEqual(["one.txt", "two.txt"]);
    expect(files[1]?.hunks[0]?.lines).toEqual([
      { type: "del", text: "two", oldNo: 1 },
      { type: "add", text: "TWO", newNo: 1 },
    ]);
  });

  it("reads a path that contains a space", () => {
    const files = parseUnifiedPatch(
      `diff --git a/my notes.txt b/my notes.txt
--- a/my notes.txt
+++ b/my notes.txt
@@ -1 +1 @@
-one
+two
`,
    );
    expect(files.map((file) => file?.path)).toEqual(["my notes.txt"]);
  });
});
