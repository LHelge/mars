---
id: "2puhk"
title: mars-deploy run says "pinned" rather than "up to date" while a pin is set
status: open
priority: P3
created: "2026-09-24T17:31:42.486490564Z"
updated: "2026-09-24T17:31:42.486490564Z"
tags:
  - deployment
  - implementation
parent: "2uqww"
---

Owner: implementation. Discovered while enabling automatic updates (zkpz7) on 2026-09-24.

With the first install's pin still set, `mars-deploy run --dry-run` printed "up to date (sha256:95dd…)" although a newer release was promoted. `cmd_run` takes the pin as the target and never looks at `mars-deploy:main`, so the wording is true of the pin and misleading about main. The operator read it as "nothing newer exists" and went looking at the packages.

Change `cmd_run` so a pinned target that is already installed logs "pinned to <release>; the promoted release is not followed (mars-deploy unpin)" and records the check as `pinned`, and so `--dry-run` under a pin says the same. `status` already shows the pin. Keep the exit status 0.

Acceptance: a transition test for "pinned, newer promoted, run" asserts the message; README.md "Automatic deployments" operator table describes `run` under a pin. References: `deploy/bin/mars-deploy` (`cmd_run`, `checked`); `scripts/release/test-transitions.sh`.