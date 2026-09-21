---
id: auzv5
title: Parse unified patches by hunk line counts so content lines starting with "-- " or "++ " survive, and handle quoted paths
status: done
priority: P1
created: "2026-09-21T10:51:33.387322477Z"
updated: "2026-09-21T17:01:02.884930241Z"
tags:
  - frontend
  - orchestrator
  - technical-review
  - bug
  - git
parent: "579dz"
attempts: 1
---

Problem: parseUnifiedPatch (frontend/src/utils/diff.ts:209-290) tests for `--- ` and `+++ ` file headers before it handles the hunk body and without asking whether a hunk is open. Reproduced by running the parser on hand-written patches:
- Deleting the SQL/Lua/Haskell comment `-- old comment` produces the patch line `--- old comment`. The parser drops it and does not advance oldNo, so every later old-side line number in the hunk is wrong and PatchFileBlock's "N changed" disagrees with the +/- counts in the file list. This repository's own .sql migrations hit it.
- An added line `++ b` arrives as `+++ b`, takes the `file.path = path` branch and renames the file, so DiffBody's block map and `key={file.path}` use the wrong path and clicking the row in the file list does nothing.
- Quoted paths: the backend runs --name-status/--numstat with -z (unquoted) but the patch itself without -z and without core.quotePath=false (orchestrator/src/git/diff.rs:106-108). For a non-ASCII or special-character name git emits `diff --git "a/f\303\266o.png" "b/..."`, the header regex does not match, `file` stays on the previous file, and the following `Binary files ... differ` marks that previous, perfectly textual file as binary (ChangesFileList shows `bin`, PatchFileBlock hides its hunks) while the real binary file gets no block. For a quoted text file headerPath keeps the quotes and octal escapes, so the patch path never equals diff.files[].path and scroll-to-file silently does nothing.
- Same off-by-one family: components/CollapsibleLines.tsx:32-34 counts a trailing newline as a line (40 lines ending in "\n" read "Show all 41 lines" and expanding reveals one empty line); utils/diff.ts splitLines already has the right rule.

Acceptance: `---`/`+++` are headers only while no hunk is open; better, read exactly the old/new counts the `@@` header declares (HUNK already matches them, count omitted means 1) before looking for headers again. Any `diff --git ` line resets the current file even when its paths cannot be parsed, and C-style quoted paths are unquoted. The orchestrator passes `-c core.quotePath=false` on the patch run (git is shelled out through src/git/, ADR 0011) with a real-repository test for a non-ASCII file name and a new binary file following a text file. CollapsibleLines reuses splitLines. Unit tests for: deleted `-- x` line, added `++ x` line, "\ No newline at end of file", empty file, mode-only change, hunk header without counts, CRLF, quoted path, binary after text.

References: frontend/src/utils/diff.ts; frontend/src/components/git/DiffBody.tsx and ChangesFileList.tsx; frontend/src/components/CollapsibleLines.tsx; orchestrator/src/git/diff.rs. Contract: SPEC.md, diff endpoints and "Frontend"; ARCHITECTURE.md, "Git model"; CLAUDE.md "Git tests use real bare repositories".