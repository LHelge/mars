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
  const repo = name.endsWith(".git") ? name.slice(0, -".git".length) : name;
  if (owner === "" || repo === "") {
    return null;
  }
  return { owner, repo };
}

/**
 * The compare page for `remote_branch` against `target`, or `null` when the
 * project does not live on GitHub. Ref names are kept as git spells them:
 * `session/<id>` compares as a path, not as `session%2F<id>`.
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
  return `https://${HOST}/${parsed.owner}/${parsed.repo}/compare/${target}...${remoteBranch}?expand=1`;
}
