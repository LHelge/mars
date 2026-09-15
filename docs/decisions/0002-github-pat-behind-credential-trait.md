# 0002. GitHub PAT behind a credential-provider trait

Status: accepted

## Context

The orchestrator needs to clone, fetch and push private repositories. Three options were considered:

1. A personal access token (fine-grained, scoped to one repository) stored as a project secret.
2. A GitHub OAuth App: users log in with GitHub and the orchestrator uses their user token.
3. A GitHub App: installation tokens minted from the app's private key, a bot commit identity, user-to-server OAuth for UI login, and webhooks.

The GitHub App is the right end state: short-lived tokens, per-installation permissions, commits attributed to a bot, and webhooks for future triggers. It is also the most setup for a self-hosted single-team deployment, and it has no equivalent for non-GitHub remotes.

## Decision

v1 uses a fine-grained PAT stored as a project-scoped, orchestrator-only secret named `GIT_CREDENTIAL`. All git remote operations obtain credentials through a `GitCredentialProvider` trait:

```rust
#[async_trait]
pub trait GitCredentialProvider: Send + Sync {
    /// Returns a header value or askpass credential valid for at least `min_ttl`.
    async fn credential_for(&self, project: &Project, min_ttl: Duration) -> Result<GitCredential>;
    /// Identity to use for commits made by the orchestrator (merges).
    fn commit_identity(&self, project: &Project) -> CommitIdentity;
}
```

The PAT implementation returns the token as an `http.extraHeader` value. A future `GitHubAppCredentialProvider` mints installation tokens and returns the same type. Callers never see which implementation they use.

## Consequences

- Rotating to a GitHub App later touches one module and adds tables for installations; no caller changes.
- A PAT is long-lived; it is only ever held by the orchestrator, never by a session container, and is passed to `git` through `-c http.extraHeader=` on the child process, never in argv URLs or on disk.
- User-to-server OAuth login is out of scope until the GitHub App lands; login is username and password.
