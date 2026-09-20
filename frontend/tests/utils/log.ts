// Reading the invite and password-reset links out of the orchestrator's log.
//
// With `RESEND_API_KEY` unset the orchestrator uses `LogEmailClient`, which
// writes the whole message — link and token included — at `info`. That is the
// one sanctioned exception to the secret-logging rule (ADR 0026, `SPEC.md`,
// "Users"), and it is how a browser scenario gets a usable invitation without
// a mail provider.

import { readFileSync, statSync } from "node:fs";

import { baseUrl, orchestratorLogPath, sleep } from "./env";

/** How long `readLoggedLink` keeps looking before giving up. */
const LINK_TIMEOUT_MS = 15_000;
const LINK_POLL_MS = 250;

/** Characters a token can be made of; the URL-safe unreserved set. */
const TOKEN_PATTERN = "[A-Za-z0-9._~-]+";

export type LoggedLinkKind = "invite" | "reset-password";

/**
 * The log's current byte length, to be passed back as `sinceOffset`. Taken
 * *before* the request that sends the mail, so a resent invitation finds its
 * own link and not the previous one.
 */
export function logOffset(): number {
  try {
    return statSync(orchestratorLogPath()).size;
  } catch {
    // The stack creates the file on its first line; nothing logged yet is 0.
    return 0;
  }
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Waits for the link of the message sent to `email` after `sinceOffset` and
 * returns it.
 *
 * The recipient is looked for first and the link only after it, because a busy
 * log interleaves messages; the body arrives in the record's `text` field,
 * which spans several lines in the pretty format and carries escaped newlines
 * in the compact one, so the search runs over the raw bytes of the tail rather
 * than line by line. The link's origin is `PUBLIC_URL`, which the stack sets to
 * the base URL under test — not a fixed `localhost:5173`.
 */
export async function readLoggedLink(
  kind: LoggedLinkKind,
  email: string,
  sinceOffset: number,
  timeoutMs: number = LINK_TIMEOUT_MS,
): Promise<string> {
  const path = orchestratorLogPath();
  const pattern = new RegExp(
    `${escapeRegExp(`${baseUrl()}/${kind}/`)}${TOKEN_PATTERN}`,
  );
  const deadline = Date.now() + timeoutMs;

  for (;;) {
    let tail: string;
    try {
      // Sliced as bytes, because `sinceOffset` is a byte length and a
      // non-ASCII line would otherwise shift the two apart.
      tail = readFileSync(path).subarray(sinceOffset).toString("utf8");
    } catch {
      tail = "";
    }
    const recipient = tail.indexOf(email);
    if (recipient >= 0) {
      const match = pattern.exec(tail.slice(recipient));
      if (match) return match[0];
    }
    if (Date.now() >= deadline) {
      throw new Error(
        `no ${kind} link for ${email} in ${path} within ${timeoutMs} ms ` +
          `(searched ${tail.length} bytes after offset ${sinceOffset})`,
      );
    }
    await sleep(LINK_POLL_MS);
  }
}
