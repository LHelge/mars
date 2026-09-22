Secrets are values that sessions or Mars itself need but nobody should read back: tokens, keys, passwords. Mars stores them encrypted and never shows them again. You can replace a value, rename a secret, change its flag or delete it, but you can't view it. Manage them on the [Secrets](/secrets) page or on a project's **Secrets** tab.

### Scopes

Every secret lives at one scope:

- **Global**: can reach the sessions of every project.
- **Project**: can reach only that project's sessions.
- **User** (**My secrets**): can reach only the sessions its owner launches. Only you, or an administrator, can see and change your own secrets.

Any signed-in user can manage global and project secrets.

### Which secrets reach a session

A secret reaches a session only when the session's profile declares its name, in the profile's **Secrets** field. Storing a secret is not enough by itself. The one exception is the [agent credential](help:agent-credentials), which every session receives without being declared.

When a declared name exists at more than one scope, the most specific one wins:

1. the launching user's secret;
2. the project's;
3. the global one.

Mars puts the winning value into the session's container as an environment variable of that name. If no scope has the name, the session still starts and shows a warning naming what is missing.

### Orchestrator only

**Orchestrator only** marks a secret that Mars keeps for its own use, such as the project's [git credential](help:git-credential). It is never injected into a session, even when a profile declares its name. If the winning secret for a name is orchestrator-only, the session gets nothing under that name at all. A secret of the same name at a less specific scope does not take its place. An agent credential can't be orchestrator-only, because then it would authenticate nothing.

### Names

A secret's name is the environment variable the agent will see. Names use capital letters A–Z, digits 0–9 and `_`, start with a letter and are at most 128 characters long, for example `NPM_TOKEN` or `SENTRY_AUTH_TOKEN`. Renaming a secret changes the variable it arrives as, so update every profile that declares it.

### Transcripts are stored as they are

Mars does not redact transcripts. Whatever an agent prints is stored and shown in full, and so is whatever you paste into a message. If a command in a session prints an environment variable or a key file, the value ends up in the transcript, in the event history and in their backups. Encrypting the secrets does not cover those copies. So don't paste credentials into messages, and don't give a session a secret it has no use for.
