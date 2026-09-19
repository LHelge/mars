---
id: zeccj
title: "Architecture: a secret's encrypted identity is owned by one type"
type: epic
status: done
priority: P2
created: "2026-09-17T19:59:47.357728130Z"
updated: "2026-09-19T19:33:34.433004881Z"
tags:
  - orchestrator
  - secrets
  - architecture
depends_on:
  - naqhy
---

## Why

The architecture survey (2026-09-17) found that `secrets/crypto.rs` claims sole ownership of the AAD (`scope:scope_id:name`) while four call sites (`service.rs` create and rename, `resolve.rs`, `git_credential.rs`) rebuild it from loose columns. The envelope exists in four shapes (`EncryptedValue`, `WrappedKey`, `KeyVersionSample`, six loose arguments to `SecretRepository::rewrap`). `crypto::rewrap` has no production caller because `rotation.rs` reimplements it with different error semantics. The keyring queries the repository at boot (`verify_against_db`), an inversion that exists to serve one call. And the seam that will actually carry a secret into a container drops the zeroize guarantee: `ResolvedSecrets.env` holds `Zeroizing<String>`, `SessionSpecInput.secrets` holds `String`, and `GitCredential.token` is a plain `String` with a manual `Drop`.

## Scope

- One sealed-envelope type owns the AAD and the wrapped-key triple; sealing, opening, resealing and rewrapping are its behaviour; the service, resolver, git credential and rotation all go through it.
- The keyring stays pure; boot verification is a startup step that uses the repository.
- Resolved values stay zeroizing until the engine or git needs the bytes.
- Scope, limit and name validity and the `SecretMeta` projection each have one home; the HTTP tests cover transport only.

## Documents

`ARCHITECTURE.md` "Secrets" (envelope diagram, "Keyring", "Rotation", "Resolution at launch"), "Session container specification"; `docs/data-model.md` `secrets`, `secret_uses`; ADRs 0002, 0006.

## Acceptance criteria

- [ ] No code outside the envelope type builds an AAD string or touches `data_key_wrapped`/`data_key_nonce`/`key_version` as loose values.
- [ ] `rotation.rs` contains no unwrap/wrap sequence of its own.
- [ ] `keyring.rs` imports nothing from `repositories`.
- [ ] A resolved secret value is `Zeroizing` from the resolver through `ContainerSpec` construction.

## Out of scope

The launcher and the git credential provider's consumers (session and git epics), which take the zeroizing types as given.