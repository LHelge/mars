# 0036. Agent credentials belong to the backend and are resolved implicitly

Status: accepted.

## Context

Every session needs a model credential, and for Claude Code that is `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`. Until now it was an ordinary secret: the user had to know the exact name, store it, and then add the same name to every profile's `secrets` list. Nothing told them beforehand that a step was missing; a forgotten or misspelt name produced a `launch_warning`, a container, a 401 from the API and a parked session. A session that resolved both names was refused at launch, which is also late.

The first proposal was to let a secret be tagged as an agent credential and to give the profile a selection box that picks one. Two things speak against it. A profile belongs to the project and is used by everyone in it, while the credential is usually personal (a subscription token): a profile that points at one secret row either runs a teammate's session on the selecting user's token or does not resolve for the teammate at all, and it bypasses the scope order (`global`, `project`, launching `user`) that already answers "whose credential" per launch. And the tag carries no information: the name is dictated by the CLI, so a tagged secret under any other name would not authenticate, and a tag is a second fact that can disagree with the name.

## Decision

The agent backend declares the secret names it authenticates with (`AgentBackend::credential_names`). The launcher resolves them for every session of that backend without the profile listing them, and a profile's `secrets` list may no longer contain one.

A scope holds at most one credential per backend, enforced where the secret is written (409) and by a partial unique index. At launch the credential at the most specific scope wins whichever of the backend's names it carries, so exactly one is injected and the launch-time refusal of "both" is removed. A credential cannot be `orchestrator_only`.

Which credential a launch would use is answerable before launching: `GET /projects/{pid}/agent-credentials` returns, per backend, the name, scope and id of the secret that would win for the calling user, never a value. The Secrets page creates credentials through a guided form that writes an ordinary secret under the right name, and the profile editor and the launch form show the answer of that endpoint.

A launch with no credential is still not refused by the server. The stub image and an image that carries its own authentication need none, so the server keeps reporting it as a `launch_warning`; the launch form warns before the fact and leaves the decision to the user.

A per-profile choice of credential *kind* (automatic, API key, subscription token) would let one profile bill an API key while the user also has a subscription token. It resolves per user and so does not have the defect of selecting a row, but nobody needs it yet; it is one column if a need appears.

## Consequences

Authenticating agents is one step, in one place, with the name never typed. The secrets table, the envelope and the audit trail are unchanged: a credential is a secret, `secret_uses` records its injection like any other, and the authentication-failure event still names the variable and its scope.

The secrets resolver stays ignorant of credential names; it is handed the backend's list beside the profile's. The write path of `/api/secrets` does learn them, through the backends, to enforce one per scope and to report `credential_for` in `SecretMeta`. A second backend adds its names in its adapter and one partial unique index in a migration; credentials of different backends coexist at one scope.

The migration removes credential names from existing `agent_profiles.secrets` and fails if a scope already holds both Claude credentials, which has to be resolved by hand by deleting one.
