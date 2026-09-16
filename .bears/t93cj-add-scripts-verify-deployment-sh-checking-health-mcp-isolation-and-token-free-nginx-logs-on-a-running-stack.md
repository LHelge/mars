---
id: t93cj
title: Add scripts/verify-deployment.sh checking health, MCP isolation and token-free nginx logs on a running stack
status: open
priority: P2
created: "2026-09-16T20:44:09.455465426Z"
updated: "2026-09-16T20:47:04.635331130Z"
tags:
  - infra
  - tests
depends_on:
  - sgg2e
parent: "5czwa"
---

## Summary
Turn two of the epic's acceptance criteria into a repeatable script instead of a manual checklist: `scripts/verify-deployment.sh` runs against a started compose stack and asserts that `/api/health` is 200 through nginx, that the MCP listener is unreachable from the host, that from a throwaway container on `mars-frontend` it answers only with a bearer challenge while from one on `mars-sessions` the `orchestrator` alias resolves, and that nginx access logs for `/ws/` and `/tasks/stream` requests contain no query string. Both walkthrough tasks run it on their engine, and it stays as an operator smoke test.

## Documents
- Epic acceptance criteria: "nginx access logs for `/ws/` and `/tasks/stream` contain no query strings"; "The MCP listener is unreachable from the host and from the frontend network".
- `ARCHITECTURE.md` "Networks" (MCP listener "binds on all interfaces but is only reachable through `mars-sessions` because nginx never forwards to it and the host does not publish its port"; "Nothing on the host or the frontend network can reach a session container"), "MCP design" (path `/mcp`, `MCP_PORT` 7001, bearer auth → 401 without a token).
- `SPEC.md` "Authentication" (`?token=` on stream endpoints; "nginx must not log query strings for those locations"), "WebSocket: session stream", "SSE: task stream", "Health".
- `README.md` "Running it" (this script is referenced from "Start"), "Configuration" (`HTTP_PORT`, `MCP_PORT`, `API_PORT`).

## Acceptance criteria
- [ ] `scripts/verify-deployment.sh` is `bash` with `set -euo pipefail`, takes `ENGINE=podman|docker` (default: `podman` if available else `docker`) and reads `HTTP_PORT` (default 8080), `MCP_PORT` (7001), `API_PORT` (7000) from `.env` if present, and prints one `ok`/`FAIL` line per check, exiting non-zero on any failure.
- [ ] Check 1 (health): `curl -fsS http://127.0.0.1:${HTTP_PORT}/api/health` returns 200 with `"orchestrator":true`.
- [ ] Check 2 (host isolation): `curl -s --max-time 3 http://127.0.0.1:${MCP_PORT}/mcp` and `http://127.0.0.1:${API_PORT}/api/health` both fail to connect (curl exit 7); a successful connection on either is `FAIL` (a port got published).
- [ ] Check 3 (frontend network): `${ENGINE} run --rm --network mars-frontend ${CURL_IMAGE} -s -o /dev/null -w '%{http_code}' http://orchestrator:${MCP_PORT}/mcp` prints `401` (or `404` while the MCP placeholder router is still in place; see notes) and never `200`; `http://orchestrator:${API_PORT}/api/health` from the same network prints `200`. The MCP listener binds all interfaces, so TCP reachability from `mars-frontend` is expected; the check proves that nothing there can use it without a session token (the documented meaning of "unreachable from the frontend network"; see notes).
- [ ] Check 4 (sessions network): `${ENGINE} run --rm --network mars-sessions ${CURL_IMAGE} -s -o /dev/null -w '%{http_code}' http://orchestrator:${MCP_PORT}/mcp` prints `401` (or `404` with the placeholder), proving that `MCP_URL`'s default hostname resolves on the internal network and the listener is reachable there.
- [ ] Check 5 (log hygiene): issue `curl -s -o /dev/null "http://127.0.0.1:${HTTP_PORT}/ws/sessions/00000000-0000-0000-0000-000000000000?after=0&token=VERIFY-CANARY-TOKEN"` and `".../api/projects/00000000-0000-0000-0000-000000000000/tasks/stream?token=VERIFY-CANARY-TOKEN"`, wait 1 s, then read the nginx access log through `docker compose logs --no-log-prefix nginx` or `podman-compose logs nginx` and assert the last 50 lines contain both paths and do **not** contain `VERIFY-CANARY-TOKEN`, `token=` or `?`. A control request `/api/health?x=CANARY` **does** log `CANARY` (proves the grep reads the right log and only the two locations are query-free).
- [ ] The script never prints or needs real credentials; the canary token is a fixed fake string.
- [ ] `README.md` "Start" gains one sentence: after `up -d`, `scripts/verify-deployment.sh` checks health, network isolation and log hygiene. `ARCHITECTURE.md` "Networks" gains one clarifying sentence: containers on the frontend network (nginx, Postgres) can open the MCP port but hold no session token; the host and the browser cannot reach it at all.

## Implementation notes
- Files: `scripts/verify-deployment.sh` (executable), `README.md`, `ARCHITECTURE.md`.
- On check 3: the epic criterion "unreachable from the frontend network" cannot hold at the TCP level while the listener binds `0.0.0.0` and the orchestrator sits on both networks (`ARCHITECTURE.md` "Components"). The script therefore verifies the documented meaning (no port published, nginx never forwards, bearer required) and the `ARCHITECTURE.md` sentence above makes that meaning explicit. State this interpretation in the PR.
- `CURL_IMAGE` defaults to `curlimages/curl:latest`; pull it explicitly before the `--network mars-sessions` run because that network has no egress (`internal: true`). Allow the override for air-gapped hosts.
- The compose logs command differs by tool: `docker compose logs` vs `podman-compose logs`; branch on `ENGINE`. If `podman compose` (the wrapper) is installed, either works.
- Idempotent: running twice in a row passes twice; the canary lines accumulate harmlessly.
- `HOSTRUN=1` switch (used by the P3 host-run task): inverts check 2 (7000/7001 open on loopback is expected) and skips check 4.

## Edge cases
- The stream endpoints reject the fake token with 401 (or 404 for the unknown id); either is fine, the check only inspects the log line.
- If a later change moves the nginx log destination off stdout, `compose logs` is empty: the check fails loudly with `no nginx log lines found`, which is the right outcome.
- Until the MCP epic lands, the scaffolding placeholder router answers 404 on `/mcp`; checks 3 and 4 accept `404` with a `warn` line and `401` with `ok`; `200` is always `FAIL`.

## Testing
- Run on the Podman stack and the Docker stack (walkthrough tasks); `shellcheck scripts/verify-deployment.sh` clean.
- No cargo/npm chains involved.

## Documentation
- `README.md` "Start" (one sentence) and `ARCHITECTURE.md` "Networks" (one clarifying sentence), same commit.

## Assumes from other epics
- "MCP server and agent tools": `/mcp` without a bearer answers 401 (`ARCHITECTURE.md` "MCP design").
- "Real-time delivery": the two stream paths exist; before that they return 404 through the proxy, which is still logged and still checked for query strings.