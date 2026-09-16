# 0024. Retain the fixed administrator credentials for controlled setup

Status: accepted. Reaffirms the bootstrap choice in ADR 0013.

## Context

The documentation review proposed replacing the fixed administrator password with an operator-supplied initial-password setting. The operator controls the setup of each new instance and accepts the existing bootstrap flow.

## Decision

Keep `admin` / `changeme` and the mandatory first-login password change. Initial setup is under the operator's control and is completed before the instance is made available to other users. Do not add an initial-password environment variable or a setup wizard for v1.

## Consequences

The existing seed, login and invitation contracts remain unchanged. This deployment assumption resolves the review finding without adding bootstrap functionality.
