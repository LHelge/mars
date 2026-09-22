// What `Sync` does, said the same way on both buttons that offer it: the
// session view's actions and the branch panel of a session never synced
// (`ARCHITECTURE.md`, "Git model", Fetch-back: the mirror fetches the work
// clone's `session/<id>` branch, so only committed work travels).

/** The `title` of every `Sync` button. */
export const SYNC_TITLE =
  "Copy this session's commits into the project mirror (uncommitted changes are not included).";
