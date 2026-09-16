---
id: h3e43
title: Verify the claude image end to end with real credentials and record a real stub fixture
status: open
priority: P2
created: "2026-09-16T20:30:37.335119672Z"
updated: "2026-09-16T20:30:37.335119672Z"
tags:
  - images
  - agent
  - tests
depends_on:
  - "64jkc"
  - t2ecg
parent: deex5
---

## Summary
Run the built claude image by hand with a real `ANTHROPIC_API_KEY` (or `CLAUDE_CODE_OAUTH_TOKEN`) using the exact conversational and ephemeral argv from `ARCHITECTURE.md`, confirm the pinned CLI accepts every flag (in particular `--permission-prompts none`), that stdout lands in `stream.jsonl`, that `SIGINT` through `kill` ends the turn with a `result`, and that `--resume` works across two containers on the same `CLAUDE_CONFIG_DIR`. Save the scrubbed recording as the stub's default fixture, replacing the hand-authored one, and note findings for the adapter epic. This is a manual, credentialed task that never runs in CI.

## Documents
- `ARCHITECTURE.md` "Claude Code invocation" (full argv; 2.1.259+ for `--permission-prompts none`; credentials paragraph: never both variables), "Input encoding" (stdin shape), "Stop semantics", "Session image", "Storage" (`CLAUDE_CONFIG_DIR/projects/-session-work/<id>.jsonl`)
- `docs/open-questions.md` items 1, 5, 6, 7, 9 (owned by the adapter and engine epics; this task only records observations, it does not resolve or delete entries)
- `CLAUDE.md` rule 3 (no real credentials anywhere in the repository)

## Acceptance criteria
- [ ] A short procedure in `images/claude/VERIFY.md` (≤ 40 lines) lists the exact commands used: build with the pinned version, `podman run -i` with `--user 1000:1000 --userns=keep-id:uid=1000,gid=1000`, the session-equivalent binds, `-e CLAUDE_CONFIG_DIR=<path>` and `-e ANTHROPIC_API_KEY` read from the operator's shell (never written to a file in the repo), the conversational argv, two stdin messages, `podman kill --signal=SIGINT`, then a second run with `--resume <session_id>` on the same config dir.
- [ ] Observed outcomes recorded in that file as a checklist: flags accepted (exit is not a usage error); `system`/`init` present with `session_id` and `mcp_servers`; both turns produced `result` lines; SIGINT mid-turn produced a `result` and exit 0; `--resume` continued the conversation and its `init` reported the same `session_id`; the transcript file exists at `<CLAUDE_CONFIG_DIR>/projects/-session-work/<session_id>.jsonl`; whether `result.total_cost_usd` grew per turn or cumulatively (observation only).
- [ ] The recorded `stream.jsonl` from the conversational run, with `session_id`, `uuid`s, any path outside `/session/work`, and any value that could be a credential scrubbed (`grep -E 'sk-ant-|oauth|Bearer'` returns nothing), replaces `images/stub/fixtures/default.jsonl` while preserving the structural guarantees of the fixture task (three turns, subagent, `Edit`, `Bash`, permission denial, `stream_event` deltas); prompts used during recording are chosen to elicit exactly those (e.g. "read README.md, then use a subagent to grep for `main`, then edit a file and commit, then try to run a tool that the settings deny").
- [ ] `images/stub/fixtures/README.md` is updated to say "recorded from Claude Code <version> on <date>" and lists what was scrubbed.
- [ ] Any flag rejection or behavioural surprise is filed as a new Bears task linked to the "Claude Code agent backend and event translation" epic (or the "Container engine adapter" epic for signal delivery), not fixed here; if `--permission-prompts none` is rejected, bump `ARG CLAUDE_CODE_VERSION` in the claude Dockerfile task's file and re-run.
- [ ] The stub unit tests and the smoke test still pass with the new fixture.

## Implementation notes
- Files: `images/claude/VERIFY.md` (new), `images/stub/fixtures/default.jsonl` (replaced), `images/stub/fixtures/README.md`.
- To make the CLI deny a tool, put a `.claude/settings.json` with a `permissions.deny` entry in the throwaway work directory; that is the documented source of `permission_denied` messages.
- Use a throwaway git repository in `<tmp>/work` with a `README.md` so `Read`/`Edit`/`Bash git commit` have something to act on; configure `user.name`/`user.email` with obviously fake values.
- Scrub with a small `jq`/`python3` one-liner kept in `VERIFY.md`; do not hand-edit JSON.
- Cost: a few cents; note the model used.

## Edge cases
- If the CLI refuses to run as uid 1000 without a writable `HOME` (`/session/home` bind), that is a contract bug in the claude image task; fix it there and note it.
- If OAuth is used, `claude setup-token` output must never be pasted into the repository; pass it through the environment only.
- If `Init: true` turns out to be needed for SIGINT (item 7), record it and file the task against the engine adapter epic.

## Testing
- Manual; evidence is the checklist in `VERIFY.md`. Automated coverage of the resulting fixture comes from the stub unit tests and `images/smoke-test.sh`.

## Documentation
- `images/claude/VERIFY.md` and `images/stub/fixtures/README.md`. Main documents are not changed by this task; observations that affect `ARCHITECTURE.md`/`SPEC.md` go to the adapter epic's tasks.

## Assumes from other epics
- none (this task can run before the adapter exists; the adapter epic's live probe later formalises the same checks in Rust).