// The console palette, in the form xterm.js can take.
//
// `src/index.css` states the tokens in `oklch()`, which xterm's colour parser
// does not read, and an `ITheme` is a plain object rather than CSS, so the
// terminal cannot inherit them. These are the same six tokens converted to
// sRGB: `console-bg`, `console-text`, `console-muted`, `console-raised`,
// `console-border` and `console-accent`, dark and light. A token that changes
// in `index.css` changes here in the same commit.
//
// Only the surface colours are set. The sixteen ANSI colours are left at
// xterm's defaults: they are the palette a program inside the container picked,
// not ours to restate.

import type { ITheme } from "@xterm/xterm";

/** `--font-mono` from `src/index.css`. */
export const CONSOLE_MONO =
  'ui-monospace, "SF Mono", "JetBrains Mono", "Fira Code", Menlo, Consolas, "Liberation Mono", monospace';

const DARK: ITheme = {
  background: "#0f1216",
  foreground: "#e3e5e8",
  cursor: "#3db6cf",
  cursorAccent: "#0f1216",
  selectionBackground: "#2f333a",
  selectionForeground: "#e3e5e8",
};

const LIGHT: ITheme = {
  background: "#f3f5f8",
  foreground: "#1e2228",
  cursor: "#00778d",
  cursorAccent: "#f3f5f8",
  selectionBackground: "#d1d4da",
  selectionForeground: "#1e2228",
};

/** The theme for the scheme the room is in; see `prefers-color-scheme`. */
export function terminalTheme(light: boolean): ITheme {
  return light ? LIGHT : DARK;
}
