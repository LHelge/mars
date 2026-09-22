// The credential names of each agent backend, and the human labels the UI
// shows instead of them (`SPEC.md`, "Frontend", Agent credentials; ADR 0036).
//
// This is the only module in the frontend that spells a credential name. The
// guided form writes the name from this table, the section reads it back out
// of `SecretMeta.credential_for`, and nothing else in the UI has to know that
// `CLAUDE_CODE_OAUTH_TOKEN` is a thing a user could have typed.
//
// It holds no JSX so that both the form and the section can import it and a
// test can assert the table without rendering anything.

import { AGENT_BACKENDS } from "../types";
import type { AgentBackend, SecretScope } from "../types";

export interface AgentCredentialKind {
  /** The secret name, exactly as the CLI expects it in the environment. */
  name: string;
  /** What the user picks and what a row is listed under. */
  label: string;
  /** Where the value comes from and what it costs, in one line. */
  hint?: string;
}

/**
 * One entry per credential a backend authenticates with, in the order the form
 * offers them: the subscription token first, because it is what a Pro or Max
 * subscriber already has.
 */
export const AGENT_CREDENTIALS: Record<AgentBackend, AgentCredentialKind[]> = {
  claude: [
    {
      name: "CLAUDE_CODE_OAUTH_TOKEN",
      label: "Claude subscription token",
      hint: "Printed by `claude setup-token`; needs a Pro or Max subscription.",
    },
    {
      name: "ANTHROPIC_API_KEY",
      label: "Anthropic API key",
      hint: "Created in the Anthropic Console; billed per use.",
    },
  ],
};

/** Every credential of every backend, flattened; the label lookup's source. */
export const ALL_AGENT_CREDENTIALS: AgentCredentialKind[] =
  AGENT_BACKENDS.flatMap((backend) => AGENT_CREDENTIALS[backend]);

/**
 * The label for a credential name, or the name itself when it is not one —
 * a row whose `credential_for` the server set under a name this build does not
 * know is still shown, under the only thing there is to call it.
 */
export function labelForCredential(name: string): string {
  return (
    ALL_AGENT_CREDENTIALS.find((entry) => entry.name === name)?.label ?? name
  );
}

/** What the guided form's third field offers. */
export type AppliesTo = "me" | "project" | "everyone";

/**
 * The `scope`/`scope_id` of `POST /secrets` for one `Applies to` choice
 * (`SPEC.md`, "Secrets"): the caller's own user scope needs no id, a project
 * carries the project's, and everyone is the global scope.
 *
 * A `project` choice with no project selected has no scope to write to and
 * answers null; the form keeps its button disabled in that state.
 */
export function credentialScope(
  appliesTo: AppliesTo,
  projectId: string | null,
): { scope: SecretScope; scope_id?: string } | null {
  if (appliesTo === "me") {
    return { scope: "user" };
  }
  if (appliesTo === "everyone") {
    return { scope: "global" };
  }
  return projectId === null || projectId === ""
    ? null
    : { scope: "project", scope_id: projectId };
}
