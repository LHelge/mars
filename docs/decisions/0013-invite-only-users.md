# 0013. Invite-only users with a seeded administrator

Status: accepted

## Context

Users are trusted with everything once they exist, so how they come to exist is the whole access-control story. Options considered:

1. Open self-registration, with the first user becoming admin.
2. Self-registration gated by a configuration flag after the first user.
3. No self-registration: an administrator seeded by the first migration invites everyone else by email.
4. Admin-created accounts with temporary passwords, no email involved.

Option 1 makes a reachable login page a reachable signup page. Option 2 is 1 with a switch that will be left on. Option 4 makes the admin a password courier.

## Decision

Option 3. The `users` migration seeds an `admin` user with a fixed, documented default password and `must_change_password = TRUE`; until the password is changed every other endpoint refuses with 403. Admins invite an email address; the invite is a single-use, 7-day token delivered by email (or written to the log when email is unconfigured), and accepting it creates the user with a username and password of their choosing. There is no other way to create a user. Password reset uses the same token-by-email mechanism.

## Consequences

- Deployment has one required first step (change the admin password), which the UI enforces rather than the README.
- Email delivery (ADR 0014) is effectively required for a multi-user deployment; single-user deployments never need it.
- Every user's email is known to be deliverable because they got their invite through it, so there is no separate verification step or `email_verified` flag.
- Bulk onboarding is one invite per person; if that becomes a burden, admin-created accounts can be added as a second path without changing the model.
