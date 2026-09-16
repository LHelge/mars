# 0002. GitHub PAT behind a credential-provider trait

Status: accepted

Superseded by the current [git credential transport contract](../../ARCHITECTURE.md#git-model) for the `-c`/no-disk claim: credentials are placed in a temporary mode-0600 config selected through `GIT_CONFIG_GLOBAL`, then deleted. Credential values must not appear in argv; the provider-trait decision is unchanged.

## Context

The orchestrator must clone, fetch and push private repositories. Options considered:

1. A fine-grained personal access token scoped to one repository, stored as a project secret. Chosen.
2. A GitHub OAuth App: users log in with GitHub and the orchestrator uses their token.
3. A GitHub App: installation tokens minted from a private key, a bot commit identity, user-to-server OAuth login and webhooks.

The GitHub App is the right end state (short-lived tokens, per-installation permissions, bot attribution, webhooks) but is the most setup for a self-hosted single-team deployment and has no equivalent for non-GitHub remotes.

## Decision

v1 uses a fine-grained PAT stored as the project-scoped, orchestrator-only secret `GIT_CREDENTIAL`. All remote git operations obtain credentials through a `GitCredentialProvider` trait with two methods: `credential_for(project, min_ttl)`, returning a `GitCredential` valid for at least `min_ttl`, and `commit_identity(project)` for commits the orchestrator makes itself. The PAT implementation returns an `http.extraHeader` value; a future `GitHubAppCredentialProvider` mints installation tokens and returns the same type. Callers never see which implementation they use.

## Consequences

- Moving to a GitHub App later touches one module and adds installation tables; callers do not change.
- A PAT is long-lived. Only the orchestrator ever holds it; session containers never do.
- User-to-server OAuth login is out of scope until the GitHub App lands; login is username and password.
