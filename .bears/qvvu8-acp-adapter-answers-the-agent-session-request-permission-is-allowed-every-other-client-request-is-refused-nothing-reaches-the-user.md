---
id: qvvu8
title: "ACP adapter answers the agent: session/request_permission is allowed, every other client request is refused, nothing reaches the user"
status: open
priority: P2
created: "2026-09-21T12:21:28.692005Z"
updated: "2026-09-21T20:26:15.117819556Z"
tags:
  - orchestrator
  - agent
  - acp
depends_on:
  - huegh
parent: eydgf
---

## Summary
ACP lets the agent send requests to the client: `session/request_permission`, `fs/read_text_file`, `fs/write_text_file`, `terminal/*`. Curiosity sends none, but the adapter is shared by every future ACP agent, and an unanswered request blocks a turn forever. ADR 0033 stands: there is no prompt event and no answer input; the container is the permission boundary (ADR 0012).

## Documents
- `ARCHITECTURE.md`, "ACP adapter": the rule. ADR 0033 gets a one-line note that the ACP adapter upholds it by answering on the user's behalf.

## Acceptance criteria
- [ ] `session/request_permission` → an outbound response selecting the most permissive "allow" option offered (prefer allow-always over allow-once, by option `kind`); if no allow-kind option exists, the first option, and a `raw` event records the request.
- [ ] `fs/*` and `terminal/*` cannot arrive (capabilities not advertised); if they do, and for any unknown method with an id → JSON-RPC error `-32601`, and a `raw` event. Notifications without id are never answered.
- [ ] A permission request produces a `raw` event at most — not `permission_denied`, not a new kind.
- [ ] On recovery replay, requests before the read offset are not answered again (seam epic, restart task).

## Testing
- Unit tests over exact outbound lines; an owner test through the mock engine: scripted agent line in, response line on stdin, turn proceeds.