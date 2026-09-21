---
id: "3q5pg"
title: Validate WebSocket and SSE message shapes at the frontend boundary
status: open
priority: P2
created: "2026-09-21T10:44:47.343970291Z"
updated: "2026-09-21T10:56:47.558117130Z"
tags:
  - frontend
  - technical-review
  - typescript
  - reliability
depends_on:
  - "3rgt7"
parent: "579dz"
---

Problem: JSON.parse(data) as ServerMessage/TaskEvent asserts a compile-time type but performs no runtime validation. Syntactically valid malformed messages can reach dispatch/reducers and cause exceptions or corrupt cursors/state.

Acceptance: add focused runtime validation at the session WebSocket and task SSE boundaries before touching stores. Validate the discriminator and fields consumed by dispatch/reducers, preserve binary terminal handling, and explicitly define forward-compatible handling of unknown event/message kinds. Reject malformed messages without mutating transcript/board state or advancing cursors. Diagnostics must avoid dumping credentials or sensitive payloads. Test invalid JSON, valid JSON with wrong shapes, null/missing fields, and unknown kinds. Keep validation proportional to the existing contract rather than duplicating all business rules.

References: frontend/src/session/useSessionSocket.ts handleFrame/dispatch; frontend/src/tasks/useTaskStream.ts handleEvent; frontend/src/types/sessionSocket.ts, agentEvent.ts and tasks.ts. Contract: SPEC.md, "AgentEvent", "WebSocket: session stream", "SSE: task stream"; CLAUDE.md secret/logging rule.

Merged from the second review (2026-09-21), same finding, the two concrete failures it traced:
- Unknown AgentEvent kind (the realistic case: enum values are only ever added, and a tab stays open across an orchestrator upgrade; `launch_warning` was such an addition). foldEvent's switch (session/sessionStore.ts:375-632) has no default arm because the closed union makes TypeScript call it exhaustive, so it returns undefined and applyEvent throws a TypeError at `next.oldestSeq` inside onmessage. On the history path prependHistory throws and the whole page is lost (then wedges, see 5453d); on first load start() falls back to `after=0`, a full replay. The same cast exists on the REST history path (services/apiClient.ts parseBody), so the forward-compatible handling belongs in the fold, not only at the socket. Acceptance addition: an unknown kind renders as a `raw` row and advances the cursor; keep compile-time exhaustiveness with a `never` check plus the runtime default, and enable `@typescript-eslint/switch-exhaustiveness-check` with `considerDefaultExhaustiveForUnions`.
- TaskEvent without a numeric seq (tasks/useTaskStream.ts:140 -> taskStore.ts:138-140): `undefined <= lastSeq` is false, lastSeq becomes undefined, the next URL is `after=undefined`, the server answers 400 BAD_CURSOR, onerror refreshes (rotating the token) and reconnects with the same bad cursor forever. Guard with `Number.isSafeInteger(event.seq)` before noteEvent.
- Same class, compile-time only: `GLYPH: Record<string, string>` (session/messages/MessageRow.tsx:18) should be `Record<Message["kind"], string>`, and the `let body;` switch at :54 has no never check; `default:` arms in tasks/handoffRules.ts:84 (reviewLabel) and :181 (reviewCoverLine) would silently render a new ReviewStatus/ReviewDecision as "Unreviewed" / "Forwarding ...", a wrong statement about which code was approved.
- Small boundary guard worth adding while here: installSession writes an undefined access_token to localStorage as the string "undefined" (self-heals through a 401).