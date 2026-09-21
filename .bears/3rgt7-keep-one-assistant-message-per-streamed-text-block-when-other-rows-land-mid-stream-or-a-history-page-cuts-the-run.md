---
id: "3rgt7"
title: Keep one assistant message per streamed text block when other rows land mid-stream or a history page cuts the run
status: done
priority: P1
created: "2026-09-21T10:52:01.167569393Z"
updated: "2026-09-21T17:01:07.140188919Z"
tags:
  - frontend
  - technical-review
  - bug
  - session
parent: "579dz"
attempts: 1
---

Problem: text_delta events are persisted with a seq and `text` carries the complete text of the block (SPEC.md, "AgentEvent"). streamingId (session/sessionStore.ts:291-303) continues a streaming assistant message only when it is the last row of its scope.
- Live: text_delta x k, then anything that lands in the scope — the optimistic user message of the advertised "Interject", a user_message, git or launch_warning event, a local system row — then text_delta x m, then text. The interleaved row is now the tail, so the next delta starts a second assistant message and `text` overwrites that one with the whole block. Result: the first k deltas sit above the user message as a fragment with `streaming: true` and a blinking cursor until the next `result`, and the full answer sits below. No test covers it.
- History: loadOlder folds the older page alone (mergeHistory, sessionStore.ts:701-759, :793-813). If the page ends inside a delta run, the older state ends with a partial streaming message A while the live state already holds B, rebuilt from the later deltas and then replaced by `text`. mergeHistory reconciles only tool placeholders, so A stays forever, and because the older fold never sees a `result`, with its cursor. With PAGE_SIZE 200 and delta-dominated streams a cut inside a run is the common case for a long answer. The same applies per subagent scope.
- clearStreaming runs only on `result` (:571-607, :862-879): a kill or park mid-stream is a state_change with no result and leaves the cursor blinking permanently. addOptimisticUser sets turnActive and inputRejected never restores it, so a rejected send on an idle session leaves the button reading "Interject" until the next result.

Acceptance: a text block folds into exactly one assistant message regardless of what is interleaved and of where a history page boundary falls, in the main scope and in subagent scopes; user/system/raw rows do not end a streaming block, a tool, thinking, result or completed text does. Merging an older page whose tail is a streaming fragment of the live head drops or joins the fragment. Streaming flags clear on an ended state_change and on a fatal error as well as on result; a rejected send restores the previous turnActive. The fold stays pure and shared by the live and history paths. Fixture-based tests: interject mid-stream, git event mid-stream, page cut inside a delta run (both "live head complete" and "live head still streaming"), subagent scope, park mid-stream.

References: frontend/src/session/sessionStore.ts streamingId, clearStreaming, mergeHistory, addOptimisticUser, inputRejected; frontend/src/session/fixtures/. Contract: SPEC.md, "AgentEvent" and "Frontend", session state and transcript rendering (ADR 0022).