// Shell output arrives with the escape sequences the program wrote for a
// terminal. The transcript is not a terminal, so `SPEC.md`, "Transcript
// rendering", asks for shell tools in monospace with ANSI stripping; the
// session's live terminal (xterm.js) is where escapes still mean something.

// Operating System Commands (window titles, OSC 8 hyperlinks) run until a BEL
// or a String Terminator, so they are matched first and whole: the general
// pattern below would otherwise leave the payload behind as plain text.
/* eslint-disable no-control-regex -- the control characters are the subject
   of these two patterns, not an accident. */
const OSC =
  /[\u001B\u009B]\][^\u0007\u001B\u009C]*(?:\u0007|\u001B\\|\u009C)/g;

// CSI and the single-character escapes: colour, cursor movement, mode
// switches. This is the long-standing `ansi-regex` pattern, written with
// explicit code-point escapes so the control characters survive copying.
const ESCAPES =
  /[\u001B\u009B][[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\d/#&.:=?%@~_]+)*|[a-zA-Z\d]+(?:;[-a-zA-Z\d/#&.:=?%@~_]*)*)?\u0007)|(?:(?:\d{1,4}(?:;\d{0,4})*)?[\dA-PR-TZcf-ntqry=><~]))/g;

/* eslint-enable no-control-regex */

/**
 * `text` with ANSI escape sequences removed and CRLF normalised to LF, ready
 * for a `<pre>`. Everything else is left exactly as the program wrote it: this
 * strips control sequences, it does not interpret them, so a progress bar
 * redrawn with carriage returns still shows every pass it made.
 */
export function stripAnsi(text: string): string {
  return text.replace(OSC, "").replace(ESCAPES, "").replace(/\r\n/g, "\n");
}
