---
id: a3u6w
title: "Playwright: a Curiosity profile is created, credentialed and run on the mock model, with its coverage rows"
status: open
priority: P2
created: "2026-09-21T12:22:55.048131Z"
updated: "2026-09-21T12:22:55.048131Z"
tags:
  - frontend
  - tests
  - e2e
  - curiosity
depends_on:
  - b2vuk
  - "7bnmc"
parent: ddb8s
---

## Summary
The browser-level proof: a user picks Curiosity in the profile editor, stores an OpenRouter key through the guided form, launches a session and watches a transcript produced by the real Curiosity binary on its scripted mock model.

## Documents
- `frontend/tests/README.md`: the coverage table rows over `SPEC.md`, "User-facing features" and "Frontend" (`npm run test:e2e` checks the table before and after every run); `README.md`, "Development", "End-to-end tests": the e2e stack now needs the Curiosity image.

## Acceptance criteria
- [ ] `test:e2e:up` brings the Curiosity image into the stack (built or pulled as the stub is); the `E2E_*` knobs are unchanged. `e2e.yml` does the same in CI.
- [ ] Scenarios, arranged through `tests/utils/fixtures.ts` (`user`, `api`, `repo`, `project`, `sessions`) and `test-helpers.ts`:
  - create a profile with backend Curiosity; the image placeholder and credential notice change with the select;
  - the guided form stores `OPENROUTER_API_KEY` (an obviously fake value, rule 3); a second one at the same scope shows the conflict;
  - launch a conversational session on that profile: text, a tool call with its result, and the turn's token counters appear; a message sent mid-turn shows at once and is answered after the turn;
  - stop, send another message, the session resumes and continues.
- [ ] The mock script used by the scenarios is a fixture under `frontend/tests/` or the image, deterministic, and paced without fixed sleeps (`CLAUDE.md`: nothing sleeps for a fixed period).
- [ ] New `data-testid`s are constants in `src/utils/testIds.ts`, re-exported from `tests/utils/test-ids.ts`.

## Testing
- The full frontend chain: `npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e:up && npm run test:e2e; npm run test:e2e:down`.