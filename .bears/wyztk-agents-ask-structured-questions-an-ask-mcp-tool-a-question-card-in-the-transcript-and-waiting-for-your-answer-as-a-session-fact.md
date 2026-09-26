---
id: wyztk
title: "Agents ask structured questions: an `ask` MCP tool, a question card in the transcript, and \"waiting for your answer\" as a session fact"
type: epic
status: open
priority: P2
created: "2026-09-26T20:36:34.229777639Z"
updated: "2026-09-26T20:36:34.229777639Z"
tags:
  - orchestrator
  - frontend
  - mcp
  - sessions
  - agent
---

## Summary
Agents can only ask a person something as ordinary assistant text today. The pinned Claude Code CLI does not offer its own `AskUserQuestion` tool under Mars's headless flags (ADR 0033, fixtures `tests/fixtures/claude/2.1.274|2.1.282/prompt_kind`), so a question is indistinguishable from any other text: no options to click, and nothing in Mars knows a session is waiting on a person. With many sessions running, that is the real cost.

Give agents a Mars-owned `ask` tool on the MCP server, render it as a question card, and make "waiting for your answer" a fact the session list, project page and dashboard can show.

## Direction (agreed 2026-09-26)
- **A Mars MCP tool, not the CLI's built-in.** Backend-agnostic (fits epics fgbm3 and eydgf), and needs nothing from Claude's control protocol. Input shaped like Claude Code's `AskUserQuestion` (1–4 questions, each with a short header, 2–4 options with label and description, `multiSelect`, free-text "other" always allowed) so models use it without coaxing.
- **Non-blocking.** The tool records the question and returns at once, telling the agent to end its turn and wait. A blocking call would run into MCP request timeouts, would keep an idle-looking session open under the idle reaper, and would not survive a park or resume.
- **The answer is an ordinary `message`.** Picking options in the card sends text through the existing single input path, so ADR 0033's one-input-shape rule holds and no `answer` input kind returns. The answer text should name the question it answers so the transcript reads naturally.
- **Conversational sessions only.** An ephemeral run has nobody to answer: the tool refuses there and points at `needs_human`, which already covers that case.
- **Rendering.** The tool call already appears in the transcript as a tool use; the frontend gives `ask` its own tool renderer (a card with options, answered/unanswered state, disabled once a later user message exists) — possibly no new `AgentEvent` kind at all. Decide in the ADR.
- **"Waiting for your answer" is a session fact.** Derived or stored (decide in the ADR): an unanswered `ask` whose turn has ended. Shown as a badge in the session lists, project page and dashboard; optionally emailed like a `needs_human` escalation, with the same per-user opt-out; the idle reaper's treatment of such a session decided explicitly (park with the question still visible is the working assumption).

## Documents
- New ADR amending ADR 0033 (a question now reaches the host, through MCP rather than the CLI).
- `SPEC.md`: "MCP tool contracts" (`ask`), "WebSocket: session stream"/"AgentEvent" if a kind is added, "Frontend" (the card and the badge), "User-facing features".
- `ARCHITECTURE.md`: MCP server, session lifecycle and idle reaper, email notifications.
- `docs/data-model.md` if the waiting state is stored.
- Prompt guidance: when to ask versus decide (the Mars launch preamble of agyg2 or the templates; decide which).

## Likely subtasks (to be broken down later)
1. ADR + `SPEC.md` contract for `ask`, the waiting state and the reaper rule.
2. MCP tool handler, profile gating (on by default for conversational profiles?), refusal for ephemeral sessions; MCP tests and a conformance recording for the pinned CLI version (`scripts/mcp-record/record.sh`).
3. Waiting-for-answer state: derivation or storage, API field, event, reaper behaviour; email with opt-out.
4. Frontend: `ask` tool renderer (question card, answering sends a message), badge in session list/project page/dashboard.
5. Prompt guidance on when to use `ask`.
6. Playwright coverage over the stub image with a fixture that calls `ask`, and coverage-table rows.

## Open points for the breakdown
- New `AgentEvent` kind versus tool-renderer only.
- Stored versus derived waiting state (derivation must be cheap for list views).
- Whether the tool is always served to conversational profiles or gated in the profile's MCP allow-list.
- Behaviour when the person sends an unrelated message instead of answering.