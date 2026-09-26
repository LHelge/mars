---
id: s6n49
title: Add a frontend formatter (Prettier) with a CI check, and reformat the tree once in its own commit
status: done
priority: P3
created: "2026-09-20T22:48:22.761832688Z"
updated: "2026-09-26T19:22:33.815426753Z"
tags:
  - frontend
  - ci
  - tooling
attempts: 1
---

## Summary
The backend has `cargo fmt`; the frontend has ESLint but no formatter, so formatting is whatever each author or agent produced, and anyone whose editor or habit runs a formatter reformats unrelated lines. Observed on 2026-09-21 while working on `6sbzz`: running `npx prettier --write` on six touched files rewrote unrelated lines in `Transcript.test.tsx` and had to be reverted by hand. Give the frontend one formatter, one config and a check in the quality chain and CI, so that "formatted" is a fact and not a diff.

## Documents
- `CLAUDE.md` "Frontend conventions" (stack line) and "Code quality" (the frontend chain gains the format check, as `cargo fmt` is in the backend chain)
- `README.md` "Development" and "CI" (Frontend workflow row)
- `.claude/agents/task-implementer.md` and `.claude/skills/implement-epic/` if they spell out the frontend chain

## Acceptance criteria
- [ ] `prettier` and `prettier-plugin-tailwindcss` as dev dependencies of `frontend/`, pinned like the other tooling. The existing code already orders Tailwind classes the way that plugin does and mostly follows Prettier defaults (80 columns, double quotes, trailing commas), so the config should be close to empty — choose options that **minimise the one-off diff**, and say in the commit which were needed.
- [ ] `frontend/.prettierrc.json` and `frontend/.prettierignore` (`dist/`, `playwright-report/`, `test-results/`, generated files, `package-lock.json`; fixtures under `src/**/fixtures/` stay byte-for-byte as recorded).
- [ ] `npm run format` (`prettier --write .`) and `npm run format:check` (`prettier --check .`) in `frontend/package.json`.
- [ ] `eslint-config-prettier` added last in `eslint.config.js` so ESLint and Prettier cannot disagree; no `eslint-plugin-prettier` (formatting is not a lint rule).
- [ ] **The one-off reformat is its own commit** (`style(frontend): format the tree with prettier (<task id>)`) containing nothing else, and that commit's hash is added to a new `.git-blame-ignore-revs` at the repository root, with `git config blame.ignoreRevsFile .git-blame-ignore-revs` mentioned in `README.md` "Development". The tooling/config/CI change is a separate commit before it.
- [ ] The Frontend CI workflow runs `npm run format:check`; the frontend chain in `CLAUDE.md` becomes `npm run format:check && npm run lint && npx tsc -b && …` (or `npm run format` first, mirroring how the backend chain runs `cargo fmt` rather than `--check` — pick one and use the same form in every place the chain is written).
- [ ] The full frontend chain passes after the reformat, including Playwright.

## Implementation notes
- Do this when no frontend epic is in flight on either machine: the reformat commit conflicts with every open frontend branch. Land it between epics, and rebase nothing across it that can be finished first.
- Markdown and the repository root are out of scope: the documents use long unwrapped lines on purpose. Scope Prettier to `frontend/` by where the config and scripts live.
- Biome was the alternative (formatter and linter in one, faster). Rejected for now because ESLint is already configured with the React and TypeScript rule sets in use, and Biome has no equivalent of the Tailwind class-sorting plugin the code already conforms to. Not ADR material unless that reasoning is questioned.

## Edge cases
- Recorded fixtures and any JSON compared byte-for-byte in tests must be ignored, not reformatted.
- `index.html`, CSS with Tailwind 4 `@theme` blocks: check Prettier parses them unchanged before including them; ignore them otherwise.
- Editors: optionally add `frontend/.vscode`-free guidance only in `README.md` (format on save with the workspace Prettier); do not commit editor settings unless the repository already does.

## Testing
- `npm run format:check` is the test; run it on a clean checkout after the reformat commit and in CI.
