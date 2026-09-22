# 0044. The server pulls a promoted release bundle from GHCR on a user timer

Status: accepted, written before the implementation (Bears epic `2uqww`, task `tke9r`).

## Context

A single server should follow the newest tested `main` without a person deploying each commit, under a rootless service user, without disturbing live sessions, and without ever running a half-published release or regressing when CI runs finish out of order. The mechanism decides where credentials live and what can reach the server.

## Decision

GitHub-hosted CI tests the exact `main` commit, publishes digest-pinned images and a `FROM scratch` bundle image (`mars-deploy`: manifest, compose files, updater, units) to GHCR, and moves the bundle's `main` tag only forward along `main`'s history. A user systemd timer on the server runs a Bash-and-`jq` updater that resolves that tag, checks the candidate against its rules and applies it; a deploy epoch in `deploy/EPOCH` holds any release that needs an operator's hand (`ARCHITECTURE.md`, "Server deployment").

Rejected:

- **Push over SSH from Actions.** Puts a login to the production host in GitHub's secrets and opens an inbound path; a pull needs only a read-only package token on the server.
- **A self-hosted runner on the server.** Runs workflow code, including code from pull requests if misconfigured, next to production and its engine socket.
- **Watchtower or `podman auto-update`.** Follow one image tag each, so the orchestrator, nginx, session images and compose files can drift apart, and nothing takes a backup or checks migrations first.
- **GitHub Release assets or a `deploy` git branch as the pointer.** A second credential (contents) or git on the host, beside the package token the images need anyway.
- **An updater in Rust.** Needs a host binary or a bootstrap container with the socket; the updater is short orchestration of `podman` commands.

## Consequences

- The server needs `podman`, `podman-compose`, `jq`, `flock` and a package-read token; nothing inbound.
- Every `main` push runs the full suites before release, not the path-filtered subset.
- An update restarts the orchestrator, with ADR 0020's input-delivery limits; a down-migration is never automatic.
