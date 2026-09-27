---
id: x7agt
title: "Composer on a touch keyboard: Enter makes a newline and the button sends"
status: done
priority: P1
created: "2026-09-23T15:58:18.068594929Z"
updated: "2026-09-27T16:47:53.561048984Z"
tags:
  - frontend
  - mobile
  - sessions
depends_on:
  - cdkxp
  - a48hj
parent: yymt7
attempts: 1
---

## Summary

`src/session/Composer.tsx:176-183` sends on Enter and inserts a newline on Shift+Enter. A phone keyboard has no Shift+Enter, so a message can never contain a newline and every Enter sends. On a coarse pointer Enter inserts a newline and the Send/Interject button is the only way to send; on a fine pointer nothing changes. The IME rules stay as they are.

Implements `SPEC.md`, "Frontend", "Composer" and "Mobile layout".

## Acceptance Criteria

- [ ] With `useMediaQuery("(pointer: coarse)")` true, `keydown` Enter in the composer is not intercepted (the textarea inserts a newline); with it false the existing rule holds (Enter sends, Shift+Enter newline, composition and key code 229 skipped).
- [ ] The Send/Interject button is always visible beside the textarea at every width and has the `TAP` hit area (`TAP` from `fieldStyles.ts`, landed by the touch-controls task a48hj this depends on); the stop button likewise.
- [ ] The textarea's `MAX_LINES` cap (8) becomes 5 below `sm`, so the composer never takes more than about a third of a phone screen.
- [ ] The composer's placeholder or hint reads "Enter sends, Shift+Enter for a newline" on a fine pointer and nothing on a coarse one (a phone user expects Enter to be a newline; the hint would only take room).
- [ ] `SPEC.md`, "Frontend", "Composer": "submit on Enter (Shift+Enter for newline)" gains "on a fine pointer; on a coarse pointer Enter is a newline and the button sends"; "Mobile layout" gets the one-line summary.

## Implementation Notes

- The keydown handler reads the hook's value from a ref or from the closure; the `send` callback is unchanged. Keep the `IME_KEY_CODE` branch first: it is Safari on both pointers.
- `autoGrow` (`Composer.tsx:41-55`) takes `MAX_LINES` as a parameter rather than a module constant.
- Do not use `enterkeyhint="send"`: on a coarse pointer Enter is a newline, so the key should say so (`enterkeyhint="enter"`), which is also the default.

## Edge Cases

- A message resent from the transcript (`useResendRequest`) still focuses the textarea; on a phone that opens the keyboard, which is what the user asked for.
- A trackpad laptop with a touch screen: `pointer: coarse` is false (primary pointer is fine), so Enter sends — the rule in SPEC says so.

## Testing

- Vitest: `Composer.test.tsx` covers both pointer values through a `matchMedia` stub: Enter inserts on coarse, sends on fine.
- Playwright `@mobile`: `sessions.spec.ts` › `a phone types a two-line message and sends it with the button @mobile` — the stub image's transcript receives a message containing a newline; assert the user message renders two lines. Row in `frontend/tests/README.md` under "Composer".
- Frontend chain in full.