---
id: s787g
title: "The e2e stack script runs on macOS: Podman machine uid map, no /proc"
status: open
priority: P2
created: "2026-09-25T10:24:52.267989Z"
updated: "2026-09-25T10:24:52.267989Z"
tags:
  - frontend
  - tests
---

Found while verifying epic `xz6yq` on macOS with a Podman machine. `frontend/tests/e2e-stack.sh` only worked on Linux:
- The keep-id check read the uid map through `podman unshare`, which the remote client cannot run. On macOS it now reads the map inside the machine with `podman machine ssh -- podman unshare`.
- `pid_is_orchestrator` and `orchestrator_alive` read `/proc`, so on macOS `up` gave up on the health wait at once and `down` never stopped the orchestrator. They now go through `process_name`, `process_cwd` and `process_state`, which use `/proc` where it exists and `ps` or `lsof` otherwise.

Still needed on macOS: `E2E_API_PORT`/`E2E_MCP_PORT`, because AirPlay Receiver holds port 7000. Consider noting that in `README.md`, "End-to-end tests".