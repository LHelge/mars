# 0048. The Podman deployment runs outside podman-compose's pod; not a Podman 6 requirement

Status: accepted (Bears task `2v86y`).

## Context

podman-compose 1.6.0 puts a project's containers in a pod, `pod_<project>`, created with `--infra=false --share=`: it shares no namespace and only groups the containers. The orchestrator runs with `userns_mode: keep-id` (ADR 0035), and Podman refuses `--userns` for a container in any pod on 4.9.3 and 5.7.0 ("--userns and --pod cannot be set together"); only Podman 6 accepts it. The deployment transition tests found this on CI's Podman 4.9, although `README.md` promised 4.9+, and the development host and server, both on Podman 6, had never shown it. The options were to require Podman 6, which no Ubuntu release packages yet and which CI would have to install from an unofficial build, or to keep the services out of the pod.

## Decision

`compose.podman.yml` sets `x-podman: in_pod: false`. The services reach each other over the compose networks exactly as before, since the pod shared nothing. Podman 4.9+ stays the supported floor, and CI tests the deployment on it.

## Consequences

A server installed before this change has its containers in `pod_mars`. The first release carrying the line recreates the orchestrator and nginx outside the pod; PostgreSQL, whose configuration does not change, stays in it until something recreates it. Nothing depends on the pod, so the mixed state is harmless, and `podman pod ls` showing `pod_mars` with one container is expected. Rolling back to a release from before the change puts the orchestrator back in the pod, which works only on Podman 6. The transition tests cover the move when run on Podman 6. Docker Compose ignores `x-` keys.
