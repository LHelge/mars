// The compare link the UI offers after a push (`SPEC.md`, "Frontend",
// "Changes panel"): `https://github.com/<owner>/<repo>/compare/<target>...<remote_branch>?expand=1`.
//
// It is built in the browser from the project's `remote_url` — the orchestrator
// is not asked, and nothing but GitHub is guessed at: any other host answers
// `null` and the caller shows no link rather than a broken one.

/** Only an `https://github.com/<owner>/<repo>` remote has a compare page. */
const HOST = "github.com";

interface Repo {
  owner: string;
  repo: string;
}

/**
 * `owner` and `repo` verbatim — GitHub paths are case-sensitive in the URL bar
 * — with a trailing slash and a trailing `.git` removed. Anything that is not
 * exactly two path segments on `github.com` is not a repository URL.
 */
function parseRepo(remoteUrl: string): Repo | null {
  let url: URL;
  try {
    url = new URL(remoteUrl);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    return null;
  }
  if (url.hostname !== HOST) {
    return null;
  }

  const segments = url.pathname.split("/").filter((part) => part !== "");
  if (segments.length !== 2) {
    return null;
  }
  const [owner, name] = segments;
  // `segments.length === 2` above is what fills both; the check is what tells
  // the compiler so, and it reads the same as the empty-string refusal below.
  if (owner === undefined || name === undefined) {
    return null;
  }
  const repo = name.endsWith(".git") ? name.slice(0, -".git".length) : name;
  if (owner === "" || repo === "") {
    return null;
  }
  return { owner, repo };
}

/**
 * A ref name as a path in a URL: each `/`-separated segment percent-encoded on
 * its own, so the slashes of `session/<id>` stay slashes — GitHub compares it
 * as a path, not as `session%2F<id>` — while a `#`, `?` or `%` a git branch
 * name may legally carry cannot end the path and turn the rest of the compare
 * link into a fragment or a query.
 */
function encodeRef(ref: string): string {
  return ref.split("/").map(encodeURIComponent).join("/");
}

/**
 * The compare page for `remote_branch` against `target`, or `null` when the
 * project does not live on GitHub.
 */
export function githubCompareUrl(
  remoteUrl: string,
  target: string,
  remoteBranch: string,
): string | null {
  const parsed = parseRepo(remoteUrl);
  if (parsed === null || target === "" || remoteBranch === "") {
    return null;
  }
  return `https://${HOST}/${parsed.owner}/${parsed.repo}/compare/${encodeRef(target)}...${encodeRef(remoteBranch)}?expand=1`;
}
