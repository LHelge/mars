The git credential is the personal access token Mars uses to reach a project's remote. You enter it in the **Credential** field when you create the project, and Mars stores it as the project secret `GIT_CREDENTIAL`.

### What it is used for

Only Mars itself uses the token, for every fetch from the remote and every push to it:

- the first clone;
- the background fetch and **Fetch now**;
- a push from the project page or a session's git panel;
- the `push` tool, for a profile that grants it.

Agents never see the token. It is never put into a session's container, so nothing an agent runs can read it. Mars sends it as HTTP basic auth with the username `x-access-token`, which both GitHub and GitLab accept for a personal access token.

### Which permissions it needs

Give the token as little as will work.

| Host | Token | Permissions |
| --- | --- | --- |
| GitHub | Fine-grained token, "Only select repositories" set to the project's repository | **Contents: Read and write** (GitHub adds Metadata: Read-only on its own). Also add **Workflows: Read and write** if agents may change files under `.github/workflows/`. Without it, GitHub rejects any push that touches them. |
| GitHub | Classic token | `repo`, plus `workflow` for the same reason. A fine-grained token is the better choice, because a classic one reaches every repository you can. |
| GitLab | Project access token, or a personal access token | `read_repository` and `write_repository`. The token's role must also be allowed to push to the branches Mars pushes. |

A read-only token (Contents: Read-only, or `read_repository` alone) is enough to clone and fetch. Every push then fails, and the host's refusal is what you see.

### Public repositories

A public repository needs no token to clone or fetch, so you can leave the field empty. You do need one before the first push. Add it on the project's **Secrets** tab as a project secret named exactly `GIT_CREDENTIAL`, with **Orchestrator only** ticked.

### Replacing or rotating the token

When the token expires or you rotate it, open the project's **Secrets** tab (or [Secrets](/secrets) with that project selected). Find the `GIT_CREDENTIAL` row there and replace its value. The next fetch or push uses the new token. If a clone failed because of the old token, choose **Retry clone** on the project after you replace it.

Two things about that row:

- **Keep it orchestrator-only.** Mars ticks this for you when it stores the token. Without it, a profile that listed `GIT_CREDENTIAL` among its [secrets](help:secrets) would put the token into that profile's sessions.
- **Do not rename it.** Mars finds the token by the exact name `GIT_CREDENTIAL`. Under any other name the project has no credential: fetches of a private repository fail, and so does every push.
