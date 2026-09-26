# 0055. A Mars-owned session preamble states the git flow; not a paragraph in every template

Status: accepted (Bears task `agyg2`).

## Context

Sessions regularly tried to do more with git than commit on their own branch: check out or merge into `main`, create other branches, push. Every one of those fails or goes nowhere — the container has no credential, the project repository is mounted read-only, and Mars fetches back `session/<sid>` and nothing else (`ARCHITECTURE.md`, "Git model") — but the agent did not know that until it tried.

The seeded role prompts carried role-specific git guidance (the implementer rebases on `origin`, the resolver fetches with an explicit refspec, the merger merges through a tool), but no prompt stated the flow as a whole, and the default `claude` profile and the planner said nothing about git at all. The prompts are copied into a project when it is created (ADR 0038), so fixing a template reaches only projects created afterwards, and never a profile a person wrote.

## Decision

The orchestrator composes one fixed paragraph, owned by Mars, in front of every session's system prompt at launch: the launcher's `LaunchContext.system_prompt` is the preamble, a blank line and the profile's prompt, or the preamble alone when the profile has none. It happens in one place (`session::launcher::launch_command` through `session::preamble`), backend-agnostic, for every launch path. The text lives beside the templates as `projects/templates/preamble.md`, is reproduced verbatim in `SPEC.md`, "Session preamble", and is not part of any profile and not editable. The templates are left as they are.

Rejected:

- **The paragraph in each template.** Templates are copied, not referenced: it would reach new projects' seeded profiles and profiles created from a template afterwards, and nothing else — not existing projects, not the profiles people wrote, which are exactly the ones with no git guidance.
- **Both.** The same facts in two places, one of them frozen per project at creation, drift the first time either is reworded, and an agent told two slightly different things about the same flow is worse off than one told once.

## Consequences

- Every launch, whatever the profile, project or backend, tells the agent the same facts about its branch, the fetch-back and the absence of a push, and a change to the wording reaches every project's next launch.
- A profile's system prompt is a little less than everything the agent is told by the launcher, and the profile editor says so beside the field.
- Every launch now has a system prompt, so a backend's "no system prompt" path is reached only by a caller that builds a `LaunchContext` by hand (a test, the live probe).
- The seeded templates keep their role-specific git sentences; they agree with the preamble, and a future contradiction between the two is a template edit.
