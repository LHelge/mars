# 0057. One layout for every width, adapted at Tailwind's breakpoints and on a coarse pointer; not a mobile route tree, a PWA shell or a user-agent check

Status: accepted (Bears task `cdkxp`, epic `yymt7`).

## Context

`SPEC.md` listed "Mobile layouts beyond 'does not break'" among the v1 non-goals. The console is used from a phone all the same: to glance at a session that needs a human, answer it from the composer, move a card on the board or approve a hand-off. At a phone's width the session view put a 384 px side panel beside the transcript, the board's columns ran off the screen, and controls sized for a mouse were hard to hit with a finger. Making the console usable there needs a rule every view follows, and the shape of that rule — one layout or two, decided by what — is a choice with real alternatives.

## Decision

One layout for every width. Views adapt in place at Tailwind's own `sm` (640 px) and `lg` (1024 px) breakpoints, as prefixes in class names, so the browser restyles what is mounted and no JavaScript runs on a resize. Where the width must decide *what is mounted* or what a component starts as, one hook reads it: `useMediaQuery(query)`, a `useSyncExternalStore` over `window.matchMedia`, which follows a resize across the breakpoint. Touch is `pointer-coarse:` — a property of the input, not of the width — and only a coarse pointer gets the larger hit areas (44 px) and text controls (16 px) a finger and a phone keyboard need; a fine pointer keeps the console's density at any width. A `title` tooltip is never the only place a fact lives, since touch has no hover. The terminal stays desktop-first.

Rejected: **a separate mobile route tree or app shell** (`/m/...`, or a second `App` chosen at startup). Every view would exist twice, and every rule `SPEC.md` states about a view — its read failures, its confirmations, its live updates — would have to be kept true in both; the second copy is the one that drifts. A link copied on a desktop would have to be translated for a phone, or open the desktop view there anyway. The views differ at a phone's width in layout, not in what they do, which is exactly what breakpoints express.

Rejected: **a PWA with its own shell** (a service worker, an app-shell cache and an installable standalone mode). It is a second delivery and caching model beside nginx's, with its own staleness rules over an application whose every view is live data from the orchestrator and whose access token is short-lived; it answers "offline" and "installable", which nobody asked for, and does nothing for the layout, which is the problem. The `theme-color` metas and the safe-area insets it would have brought are taken on their own.

Rejected: **a user-agent check instead of media queries.** The user agent names a device, not a viewport: a tablet in landscape is wide, a desktop window can be narrow, a phone browser can ask for the desktop site, and user-agent strings are being frozen and reduced. It is also read once, so it cannot follow a resize or a rotation, and it cannot tell a finger from a mouse, which `pointer: coarse` can.

Rejected: **a key bar above the phone keyboard for the terminal** (Ctrl, Tab, Esc, the arrows, as mobile SSH clients draw one). A phone keyboard has none of those keys, so without one the terminal on a phone is limited to typing plain commands; but the terminal is an inspection escape hatch, not a place work is done, and a key bar is a second input model with its own sticky-modifier state, focus handling against xterm's hidden textarea and layout against the keyboard, all to serve a use nobody has asked of a phone. The terminal is kept within the layout on a phone and is otherwise desktop-first.

## Consequences

- The non-goal "Mobile layouts beyond 'does not break'" is gone from `SPEC.md`; "Frontend", Mobile layout is the rule, with one sentence per view for what a phone gets, and each view's task implements its sentence.
- `useMediaQuery` returns `false` where `matchMedia` does not exist, so jsdom renders the narrow layout: a unit test of a wide layout stubs `matchMedia`.
- Playwright has a second project, `mobile` (Pixel 7), selected by a `@mobile` tag in the scenario title; phone scenarios are their own scenarios, not the desktop suite run twice, so the run time grows by what the phone promises and no more.
