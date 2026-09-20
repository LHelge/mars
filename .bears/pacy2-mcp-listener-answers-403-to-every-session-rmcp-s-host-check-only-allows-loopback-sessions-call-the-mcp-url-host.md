---
id: pacy2
title: "MCP listener answers 403 to every session: rmcp's Host check only allows loopback, sessions call the MCP_URL host"
status: done
priority: P0
created: "2026-09-20T22:50:30.350987477Z"
updated: "2026-09-20T23:48:40.094103651Z"
tags:
  - orchestrator
  - mcp
  - bug
---

## Summary
Found on the live compose stack on 2026-09-21: the Claude CLI in a session container reported `mars-orchestrator` failed to connect with HTTP 403, "Host header is not allowed", so no Mars tool was registered and agents had no task tracker at all.

Cause: `mcp_router` builds the transport with `StreamableHttpServerConfig::default()`, and rmcp 3.4.0's default `allowed_hosts` is `localhost`, `127.0.0.1`, `::1` (DNS-rebinding protection). A session reaches the listener at the authority of `MCP_URL` — `orchestrator:7001` under compose, `host.containers.internal:7001` on a development host — which the check refuses before anything else runs. Every integration test connects over `127.0.0.1`, and the stub image never calls MCP, so nothing caught it.

## Documents
- `ARCHITECTURE.md` "MCP design" (Authentication paragraph: add the Host rule)
- `README.md` "Configuration", `MCP_URL` row

## Acceptance criteria
- [ ] The transport's `allowed_hosts` is rmcp's loopback default plus the authority of `MCP_URL` (host, and the port when the URL names one). The check stays on: it is not disabled.
- [ ] `Config::from_env` refuses an `MCP_URL` without a host, naming the variable, instead of failing later.
- [ ] Regression tests: a request whose `Host` is `MCP_URL`'s authority is served; one with a foreign `Host` is 403.
- [ ] Both clippy invocations and the test suite pass.
