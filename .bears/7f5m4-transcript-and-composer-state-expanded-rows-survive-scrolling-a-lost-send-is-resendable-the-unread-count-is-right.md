---
id: "7f5m4"
title: "Transcript and composer state: expanded rows survive scrolling, a lost send is resendable, the unread count is right"
status: done
priority: P2
created: "2026-09-21T10:53:37.547254558Z"
updated: "2026-09-21T23:08:40.833394114Z"
tags:
  - frontend
  - technical-review
  - bug
  - session
depends_on:
  - "3rgt7"
parent: "579dz"
attempts: 1
---

Problem:
- Open/closed state is component-local inside virtualised rows with overscan 8: session/tools/ToolFrame.tsx:51, SubagentGroup.tsx:28, tools/SummaryToolRenderer.tsx:18, the <details> in messages/ThinkingMessage, RawMessage and SystemMessage, and components/CollapsibleLines. Open a subagent group or a long tool result, scroll about ten rows away and back: the row remounts collapsed, while the virtualizer's size cache (keyed by message id) still holds the tall height, so the layout jumps on remeasure. A subagent the reader is following closes whenever they look elsewhere. The ▾/▸ disclosure button itself is written three times (ToolFrame.tsx:66-79, SubagentGroup.tsx:34-49, SummaryToolRenderer.tsx:23-35).
- A message sent over a socket that dies before acknowledgement stays "sending..." forever with no Resend (useSessionSocket.ts:329-341, sessionStore.ts:846-860): send trusts readyState === OPEN, true on a half-dead socket after sleep; nothing marks the optimistic message on close, and inputAccepted records nothing (it sets pending: true, which it already is, and drops the `seq` its signature declares), so accepted and never-acknowledged messages are indistinguishable. ADR 0020 accepts no delivery guarantee but says users may resend manually; only rejected messages get the button (messages/UserMessage.tsx:24-38). UserMessage's independent `pending?: boolean` and `rejected?: string` make pending-and-rejected representable.
- "Jump to latest" count: appendedAfter returns order.length when lastId is no longer in order (useStickToBottom.ts:63-69). Unpinned, the user sends (tail is client:<id>, count +1), the user_message renames the tail to e<seq> (replaceOptimistic), lastIndexOf is -1 and the pill reads e.g. "Jump to latest 413". Sending also does not re-pin, so a user scrolled up does not see their own message (Composer.tsx:124-130).
- Safari fires the IME-confirming Enter after compositionend with isComposing false and keyCode 229, so it submits (Composer.tsx:182).
- The resend hand-off takes three renders and an effect for one click (Composer.tsx:71-98 with SessionView.tsx:27-35), and `socket` is both published through SessionSocketContext and drilled as props to Composer and SessionHeader.
- tools/renderers.ts:35 registers Task/Agent -> SubagentGroup, but ToolFrame.tsx:97-101 bypasses the registry whenever message.subagent is set; in the gap between tool_call and subagent_start the body is an empty SubagentGroup showing neither input nor result. toolInput.ts:143 prints Read as `:offset-limit` although limit is a count (offset 100, limit 50 reads "100-50").

Acceptance: expanded state lives outside the rows (a small per-session UI store keyed by message id, cleared with the session store) behind one Disclosure component used by all the sites above. User-message delivery is one field (pending / accepted / confirmed / rejected) and a close marks never-accepted optimistic messages as resendable. The unread count treats a vanished id as zero growth and sending re-pins. keyCode 229 is ignored. The resend button is a plain event handler. The dead registry entry is removed so the JSON fallback covers the gap, and the Read summary prints offset–offset+limit. Tests: scroll-away-and-back keeps a row open; socket close with an unacknowledged message offers Resend; the count after an optimistic rename.

References: frontend/src/session/tools/ToolFrame.tsx, SubagentGroup.tsx, tools/SummaryToolRenderer.tsx, tools/renderers.ts, tools/toolInput.ts, messages/*.tsx, useStickToBottom.ts, Composer.tsx, SessionView.tsx, useSessionSocket.ts, sessionStore.ts. Contract: SPEC.md, "Frontend", transcript rendering and session input; ADR 0020.