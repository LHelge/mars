// The one way the frontend learns who it is signed in as.
//
// The current user lives in `services/auth` and nowhere else (`SPEC.md`,
// "Frontend", Rules): every component reads it through `useAuth()`, and every
// `GET /users/me` — the authenticated bootstrap, the refresh after an
// authorization 403, the settings page — goes through this function, which
// installs what it read. A page that kept its own copy in the query cache and
// pushed it back into the store could resurrect a stale role: a user demoted
// elsewhere would get their admin navigation back from a cached answer.

import { setCurrentUser } from "./auth";
import { getMe } from "./users";
import type { User } from "../types";

/** Reads `GET /users/me` and installs it as the current user. */
export async function refreshCurrentUser(): Promise<User> {
  const user = await getMe();
  setCurrentUser(user);
  return user;
}
