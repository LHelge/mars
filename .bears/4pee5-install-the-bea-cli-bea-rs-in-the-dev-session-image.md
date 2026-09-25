---
id: "4pee5"
title: Install the bea CLI (bea-rs) in the dev session image
status: done
priority: P2
created: "2026-09-25T16:54:21.098153175Z"
updated: "2026-09-25T16:56:15.973312074Z"
tags:
  - images
attempts: 1
---

ARCHITECTURE.md, "Session image", "The dev layer". Projects migrating to Mars track work with Bears, so `images/claude-dev` carries a pinned `bea` built with `cargo install --locked bea-rs@<ver>` into /opt/cargo/bin. The upstream linux-x86_64 release binary needs glibc 2.39 and the image is bookworm (2.36), and there is no arm64 asset, so it is compiled. Assert it at build time, probe it in images/smoke-test.sh, document it in ARCHITECTURE.md and README.md.