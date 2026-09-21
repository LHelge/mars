---
id: hv8cs
title: Give the character-by-character StreamingMarkdown test a timeout that survives a loaded machine
status: done
priority: P3
created: "2026-09-21T17:28:30.557891855Z"
updated: "2026-09-21T18:14:14.159943152Z"
tags:
  - frontend
  - test
parent: "579dz"
---

Problem: frontend/src/components/StreamingMarkdown.test.tsx › "ends a character-by-character stream in the same DOM as one render" re-renders the kitchen-sink fixture once per character, about 3.5 s of CPU on a quiet 4-core machine against Vitest's default 5 s timeout. Four task-implementer agents of epic 579dz reported it timing out (5.3–13.5 s) while siblings loaded the machine, on unmodified main as well; it passes when the machine is quiet (coordinator's verification, 645/645).

Acceptance: the test carries an explicit timeout with headroom (30 s) and a comment saying why; nothing else about it changes. Discovered from: 4srw8, 3rgt7, hqbzq, auzv5, 9m8dn.