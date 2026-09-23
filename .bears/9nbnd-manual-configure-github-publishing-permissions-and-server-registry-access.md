---
id: "9nbnd"
title: "Manual: configure GitHub publishing permissions and server registry access"
status: done
priority: P1
created: "2026-09-22T07:33:36.392186Z"
updated: "2026-09-23T11:36:56.207063697Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - tke9r
  - qeesg
  - g95nh
parent: "2uqww"
assignee: LHelge
---

Owner: Linus (manual GitHub/server account work).

Apply the package visibility/linkage and workflow permissions documented by the publishing task. Configure required main checks/branch protection where available so the promotion policy is enforceable; verify only trusted main workflows can publish/promote. Confirm the first successful CI run publishes a complete deployment matching the server CPU architecture. Do not create a production self-hosted Actions runner.

If packages are private, provision the minimum read-only package access required by the service user and store registry authentication in an explicit persistent auth file usable by noninteractive user systemd services; do not rely on a login-only or /run auth file. If public anonymous pulls suffice, document that no token is needed. Confirm the updater can fetch both the pointer/bundle and images with its documented authentication mechanism.

Acceptance: service-user noninteractive retrieval succeeds; credentials survive logout/reboot as intended, have minimum permissions and are absent from the repo/task/logs; record package names/visibility and supported platform, never tokens. References: publishing task; README.md deployment installation/registry instructions.
## Evidence and choices (2026-09-23)

- Packages: `mars-orchestrator`, `mars-nginx`, `mars-session-claude`, `mars-session-claude-dev` and `mars-deploy` are all `private` and linked to `LHelge/mars` (via the `org.opencontainers.image.source` label; checked with `gh api /user/packages`). Workflow token default is `read`; Release requests `packages: write` only in its publish and promote jobs. Only a push to main publishes (qeesg); no self-hosted runner exists.
- Branch protection: not available. Rulesets and branch protection on a private repository need GitHub Pro (the API answers 403 "Upgrade to GitHub Pro"). Linus accepted an unprotected `main` until the repository goes public, when he will protect it. Mitigation already in place: promotion refuses to move `mars-deploy:main` when history diverged, and the updater ignores any release whose sequence is not higher than the installed one. So a rewritten main stops releases rather than deploying one.
- Server registry access: a classic token with only `read:packages`, created by Linus, logged in as `mars` into the persistent `~/.config/containers/auth.json` (the runtime `/run` auth file does not exist). A non-interactive `systemd-run --user --wait --pipe podman pull ghcr.io/lhelge/mars-deploy:main` succeeds, and the pulled digest is the promoted `sha256:b52897dd…` (299a805), platform linux/amd64. Reported by Linus as expected; the token is recorded nowhere. Rotate it before its expiry.
