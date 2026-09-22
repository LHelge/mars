// Where an invitation link goes (`SPEC.md`, "User-facing features", Login and
// invites; ADR 0026). The page cannot know whether email delivery is
// configured, so it says both cases rather than guessing one. Shared by the
// panel's description and success line and the row's resend answer; a module
// of its own because a module that renders a component exports nothing else.

export const INVITE_DELIVERY =
  "It is sent by email when this instance has email configured; otherwise an operator finds the link in the orchestrator log.";
