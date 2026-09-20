// The session stream URL of `SPEC.md`, "WebSocket: session stream":
// `GET /ws/sessions/{id}?after=<seq>&token=<jwt>`.
//
// Kept in its own module so the URL can be asserted in unit tests without a
// socket, and so the token is only ever spelled out here — never logged.

export function buildSessionSocketUrl(
  sessionId: string,
  after: number,
  token: string,
): string {
  const scheme = globalThis.location.protocol === "https:" ? "wss" : "ws";
  const host = globalThis.location.host;
  return `${scheme}://${host}/ws/sessions/${encodeURIComponent(sessionId)}?after=${after}&token=${encodeURIComponent(token)}`;
}
