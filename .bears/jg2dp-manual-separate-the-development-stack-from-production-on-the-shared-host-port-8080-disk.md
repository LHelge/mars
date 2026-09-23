---
id: jg2dp
title: "Manual: separate the development stack from production on the shared host (port 8080, disk)"
status: done
priority: P1
created: "2026-09-22T21:36:38.639721171Z"
updated: "2026-09-23T06:16:58.649681376Z"
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
- 2026-09-23: access limited. No host firewall was active before (nftables, ufw, firewalld and iptables all inactive; `/etc/nftables.conf` held Arch's unused default, now kept as `/etc/nftables.conf.arch-default`). Linus replaced it with one table, `inet webapps`, that restricts only ports 3000 (another project behind the same Traefik) and 8080:
  - accept from `lo`;
  - accept from Traefik, 192.0.2.4;
  - drop everyone else, IPv4 and IPv6. The host has a ULA IPv6 address, so an `ip saddr`-only rule would have left IPv6 open.
  `nftables.service` is enabled; on this Arch version it is a oneshot without RemainAfterExit, so it reads `inactive` after loading successfully at 08:13:40. Checked with the 3000 app: curl answers from the Traefik host, other LAN machines get no answer, and this host reaches `192.0.2.50:3000` through `lo` (404 from the app). Mars itself binds `HTTP_PORT=192.0.2.50:8080` (a8gbx).
- Disk budget: about 60G for Mars (`/srv/mars` plus `/home/mars/.local/share/containers`). Five retained releases of images stay well under 20G because layers are shared; full backups of `/data` are the variable part. Rule: keep `/` below 80 % used, and move `/srv/mars` or the development cargo target directories to separate storage if development builds push past it. On 2026-09-23 `/` is 491G with 228G free (52 %).
