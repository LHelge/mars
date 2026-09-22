# 0047. Icons are Lucide, behind one concept-named map; not Heroicons, and not two sets mixed

Status: accepted (Bears task `v9rjg`).

## Context

The console drew seven Heroicons (`@heroicons/react/24/outline`) in eight files: log out, help, close, edit, delete, a lock, a warning and a few chevrons. Git is a large part of what the console does — fetch, sync, merge, rebase and push on session branches, hand-offs that carry commits — and Heroicons has no git icons at all: no branch, merge, commit or pull request. Adding icons across the nav, the git actions, the session controls, the copy buttons and the state indicators needed a set that has them.

## Decision

Icons are Lucide (`lucide-react`): the same 24px outline style Heroicons had, git, terminal and bot icons among its ~1,500, and one ES module per icon, so a named import costs that icon and not the library.

Components never import `lucide-react`. `src/components/icons.ts` maps concepts to icons — `Icon.push`, `Icon.merge`, `Icon.running`, `Icon.needsHuman` — with one size constant, `ICON_CLASS`, and an ESLint `no-restricted-imports` rule refuses the library everywhere else. The map is what keeps a concept one glyph across the console, and makes a later change of glyph, or of library, one file.

An icon always sits beside its text and never replaces a label: `aria-hidden`, coloured by the text it sits in, and an icon-only button keeps its `aria-label`.

Rejected: **keeping Heroicons**. It has nothing for a branch, a merge, a commit or a pull request, so the git actions — the part of the console with the most actions on one screen — would have stayed bare, or been given generic arrows that say nothing git-specific.

Rejected: **Heroicons plus Lucide for what Heroicons lacks**. Two sets differ in stroke width, corner radius and optical size, and a row that puts a Heroicon pencil beside a Lucide merge shows it. Two sets also means two answers to "which library has this icon", which is how a concept ends up with two glyphs; one set behind one map has one answer.

Rejected: **importing icons directly from `lucide-react` in each component**. It tree-shakes the same, but a concept would be named by its glyph at every call site (`GitMerge`, `ArrowUpFromLine`), so nothing would keep `push` the same arrow in the branch table and in its form, and the sizing would be copied class strings again.

## Consequences

- `@heroicons/react` is gone from `package.json`; `lucide-react` replaces it.
- A new icon is a new entry in `icons.ts`, named for what it means, not for what it looks like.
- `PageLayout` and `StatusBadge` are on the first-paint path and read the map, so every mapped icon is in the entry chunk: measured at about 10 KB for the map of this change (338,722 to 348,878 bytes, budget 400,000). If the map grows until that matters, the split is by first-paint need — a second map for the lazily loaded views — not a return to direct imports.
