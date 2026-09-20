// The container engine as a scenario reads it: what is still there.
//
// Everything Mars creates carries `mars.session_id` (`ARCHITECTURE.md`,
// "Engine adapter"), so a session's containers are one labelled listing. Only
// this direction is offered: a scenario asserts what the orchestrator left
// behind and never removes a container itself, because the stack shares the
// host with whatever else is running on it.
//
// `PLAYWRIGHT_ENGINE` names the binary the stack was brought up with
// (`tests/e2e-stack.sh`), so a run on Docker asks Docker.

import { execFileSync } from "node:child_process";

import { engineBinary } from "./env";

/**
 * The ids of every container — running or exited — labelled with this session,
 * as the engine lists them.
 *
 * An engine that cannot be asked at all throws with its own message; an empty
 * listing is the answer "none", which is what an ended session must leave.
 */
export function sessionContainers(sessionId: string): string[] {
  const output = execFileSync(
    engineBinary(),
    [
      "ps",
      "--all",
      "--filter",
      `label=mars.session_id=${sessionId}`,
      "--format",
      "{{.ID}}",
    ],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );

  return output
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}
