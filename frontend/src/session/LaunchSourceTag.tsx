// "Nobody launched this one." One tag, wherever a session is listed or
// headed, saying that the session came from automation rather than from a
// person (`SPEC.md`, "Frontend"; `ARCHITECTURE.md`, "Task tracker" →
// "Unattended launches").
//
// Provenance is not a state: it is fixed at creation and never changes, so the
// tag takes no colour. It is an outlined monospace chip — the shape the console
// already uses for an identifier — which reads apart from the filled state
// pill beside it without competing with it.

import { LAUNCH_SOURCE } from "../utils/testIds";
import type { LaunchSource } from "../types";
import { launchSourceLabel, launchSourceTitle } from "./launchSource";

export interface LaunchSourceTagProps {
  source: LaunchSource;
}

export function LaunchSourceTag({ source }: LaunchSourceTagProps) {
  const label = launchSourceLabel(source);
  if (label === null) {
    return null;
  }

  return (
    <span
      data-testid={LAUNCH_SOURCE}
      title={launchSourceTitle(source) ?? undefined}
      className="border-console-border text-console-muted inline-flex items-center rounded border px-1.5 py-0.5 font-mono text-xs whitespace-nowrap"
    >
      {label}
      <span className="sr-only"> launched this session</span>
    </span>
  );
}
