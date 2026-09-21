---
id: td3dh
title: "Project pages: shared-directory guard that never updates, delete that 404s its own page, profile-kind toggle that rewrites partial_messages"
status: open
priority: P3
created: "2026-09-21T10:55:09.160352674Z"
updated: "2026-09-21T10:55:09.160352674Z"
tags:
  - frontend
  - technical-review
  - bug
parent: "579dz"
---

Problem, each small and confirmed by reading:
- pages/project/SharedDirsTab.tsx:54-62 (:224, :233): `live` is queryClient.getQueriesData(...) read during render with no subscription, and while this tab shows SessionsTab is unmounted so nothing polls that list. The Sessions tab showed a running ephemeral session; the user switches to Shared directories and the session ends a minute later: Clear and Remove stay disabled with "A session is running" until the Sessions tab is revisited. The comment calls it "a courtesy, not a guard", but a disabled button is a guard. (The reverse case is harmless: the server's 409 handles it.)
- pages/project/ProjectHeader.tsx:74-77: delete invalidates the whole ["projects"] tree while the deleted project's page is still mounted. Every active query under ["projects", id, ...] refetches and 404s; /projects is a React.lazy route and navigation runs in a transition, so if the detail 404 lands first ProjectView renders NotFoundPage before the list appears. The retry-clone "adopt" logic is also duplicated between ProjectsPage.tsx:73-81 and ProjectHeader.tsx:48-52.
- pages/ProfileEditorPage.tsx: partialTouched starts false even when editing, so a stored conversational profile with a deliberate partial_messages: false that is switched to ephemeral and back gets the box checked and saved.
- pages/AcceptInvitePage.tsx:114-115 runs queryClient.clear() after `await navigate("/")`; in declarative-router mode that await is a microtask, so if Dashboard's observers mount first, clear() removes their queries and later invalidateQueries/setQueryData cannot reach them until a remount (plausible).
- pages/DashboardPage.tsx:40-45: the stretched row link relies on `position: relative` on <tr>, which WebKit has historically ignored; there every link's after:absolute inset-0 resolves to a far ancestor and the last row's link captures clicks everywhere (plausible, not verified per browser).
- pages/SecretsPage.tsx: a non-admin opening ?scope=user&scope_id=<other> gets no radio checked, no select and a 403 list; the URL is normalised in an effect after one render with the non-canonical key. (File changed after the review; re-verify.)
- pages/project/SessionsTab.tsx:84 uses keepPreviousData where the key does change but never reads isPlaceholderData, so switching All -> Failed briefly lists running rows under "Failed"; GitActionsPanel already holds the unfiltered list under the same base key, so client-side filtering would remove a second polled request.
- pages/project/profileForm.ts:20/:151: the UI's idle-timeout floor is 60 while the API's is 1, so a profile created over the API with 30 s cannot be saved from the editor until the value is raised.

Acceptance: SharedDirsTab either subscribes to the sessions list (useQuery with the same key and a refetchInterval) or keeps the warning and leaves the buttons enabled so the 409 stays the authority. Project delete removes the project's queries (removeQueries on the detail prefix) and invalidates only projects.list(); one adopt helper. partialTouched initialises from the stored profile (`profile.partial_messages !== partialMessagesDefault(profile.kind)`). AcceptInvite removes everything but the invite query before navigating. The dashboard row link does not depend on a positioned <tr>. The secrets scope URL is normalised without rendering the bad key. The Failed filter does not show placeholder rows as its own. The idle-timeout floor matches the API or the difference is documented. One focused test per behavioural fix.

References: files above. Contract: SPEC.md, "Shared directories", "Projects", "Profiles", "Frontend".