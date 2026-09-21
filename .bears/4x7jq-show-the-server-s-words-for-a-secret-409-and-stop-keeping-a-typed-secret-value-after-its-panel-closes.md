---
id: "4x7jq"
title: Show the server's words for a secret 409 and stop keeping a typed secret value after its panel closes
status: open
priority: P2
created: "2026-09-21T10:52:27.644527706Z"
updated: "2026-09-21T10:52:27.644527706Z"
tags:
  - frontend
  - technical-review
  - bug
  - secrets
parent: "579dz"
---

Problem:
- components/secrets/messages.ts:33 maps every 409 to "A secret with that name already exists in this scope." Since sk5n8, POST/PATCH /secrets also answer 409 `this scope already has an agent credential (<NAME>); replace or delete it first`. A project that holds CLAUDE_CODE_OAUTH_TOKEN and an operator adding ANTHROPIC_API_KEY (CreateSecretForm), renaming a secret to it (SecretRow), or using the guided form (secrets/AgentCredentialsSection.tsx:196 and :213, which uses the same mapper and for which this 409 is the expected refusal) is told something false and the actionable server text is discarded. SPEC.md, "Frontend" says a 409 is shown with the body's `error`.
- components/secrets/SecretRow.tsx togglePanel (:115-118) does not clear `value`; only Cancel does (:321-324). Closing the panel by clicking "Replace value" again, or switching to Rename or Uses, unmounts the textarea but keeps the plaintext in component state; it reappears when the panel is reopened. The file header promises it "lives in this component's state until the mutation settles and nowhere else".
- SecretRow delete only invalidates the list (:105-113): remove.isPending goes false on success while the row is still rendered, so all four buttons are live on a secret that no longer exists (404 on click).
- components/secrets/SecretUsesList.tsx:57-64: `limit` is in the query key with no placeholderData, so "Show more" replaces the whole list with LoadingState, the scroll jumps, and the button's own `loading={uses.isFetching}` spinner can never show.

Acceptance: a 409 shows caught.error (reword only when the body is the duplicate-name message, if the friendly wording is wanted). The replacement value is dropped whenever the replace panel is not the open one — cleanest by moving the replace and rename forms into child components that unmount with the panel, which is also the seam for splitting the 388-line SecretRow and lets them use the field component from s9rxc. A deleted row leaves the list on success (setQueryData as InvitesPanel.revoke does, or keep the mutation pending through the refetch). SecretUsesList keeps the previous rows while the larger page loads. Keep the existing `gcTime: 0` + reset() handling of plaintext in the mutation cache. Tests for the agent-credential 409 text in all three callers and for the value being gone after the panel toggles.

References: frontend/src/components/secrets/messages.ts, SecretRow.tsx, CreateSecretForm.tsx, SecretUsesList.tsx; frontend/src/secrets/AgentCredentialsSection.tsx. Contract: SPEC.md, "Secrets" (409 rules) and "Frontend", "Agent credentials"; CLAUDE.md rule 3.