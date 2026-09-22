---
id: jg2dp
title: "Manual: separate the development stack from production on the shared host (port 8080, disk)"
status: open
priority: P1
created: "2026-09-22T21:36:38.639721171Z"
updated: "2026-09-22T21:36:38.639721171Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - g95nh
parent: "2uqww"
---

Owner: Linus (manual server work). Discovered while recording g95nh.

The production server (192.0.2.50) is also the development host. On 2026-09-22 the development walkthrough stack is running under `lhelge`'s rootless Podman (`mars_nginx_1` published on `0.0.0.0:8080`, plus `mars_orchestrator_1` and `mars_postgres_1`). Traefik already forwards `https://mars.example.com` to `192.0.2.50:8080`, so today the public hostname reaches that **development** instance.

To do before the first production install (sswjj):
- Stop the development compose stack, or move it to another `HTTP_PORT`, so the `mars` user's nginx can bind 8080. Until then, check that the development instance's admin password is not the bootstrap default.
- Keep the production nginx reachable only from Traefik: `HTTP_PORT=192.0.2.50:8080` plus a host firewall rule letting only the Traefik host reach 8080, or document why the LAN-wide exposure is fine.
- Disk: `/` has 175G free. `orchestrator/target` is 86G now, and development target directories have reached ~190G before. Production keeps about five releases of images (the dev session image is ~1.9G each) plus database backups. Decide on a budget, or move `/srv/mars` or the development target directories onto separate storage, so a development build cannot fill the disk under production.
- Rootless Podman keeps storage, networks and the socket per user, so `lhelge`'s development containers and `mars`'s production containers do not share `mars-sessions`/`mars-egress` or images. Only host ports and disk are shared. E2E uses ports 5433/7000/7001 on loopback and does not collide with 8080.

Acceptance: port 8080 on 192.0.2.50 belongs to the `mars` user's nginx only (or is free for it), reachability from outside is limited as decided, and the disk budget is written down here.
## Progress

- 2026-09-22: Linus stopped the development stack with `podman-compose down`. Nothing listens on 8080 now (`ss -ltn`), so the port is free for the `mars` user's nginx. Still open: the access limit (bind address and firewall) and the disk budget.
