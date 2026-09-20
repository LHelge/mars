// Git on the host: the local upstreams projects clone from, the work clones
// sessions commit into, and the two read-only queries scenarios assert with.
//
// The orchestrator runs on the same host as these tests (`tests/e2e-stack.sh`),
// so a `file://` remote needs no credential and its `git ls-remote --symref`
// discovers `main` exactly as ARCHITECTURE.md, "Git model", describes. Session
// containers never see these repositories: they clone from the project mirror.
//
// Every command goes through `execFileSync` with an argument array — never a
// shell string — with the developer's own git configuration switched off so a
// run is the same everywhere.

import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { dataDir, randomSuffix, reposDir } from "./env";

const GIT_ENV = {
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_TERMINAL_PROMPT: "0",
};

/** The identity commits made from the host carry; obviously fake (rule 3). */
const COMMITTER = ["-c", "user.name=E2E", "-c", "user.email=e2e@example.test"];

/**
 * One git command. `stderr` is captured rather than inherited, so a successful
 * `push` does not write into the Playwright report; a failure carries it in the
 * thrown message, which is the only place it is wanted.
 */
function git(cwd: string, args: string[]): string {
  try {
    return execFileSync("git", args, {
      cwd,
      encoding: "utf8",
      env: { ...process.env, ...GIT_ENV },
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  } catch (error) {
    const stderr = (error as { stderr?: string }).stderr ?? "";
    throw new Error(`git ${args.join(" ")} (in ${cwd}) failed: ${stderr}`, {
      cause: error,
    });
  }
}

/** Writes `files` (paths relative to `root`, directories created as needed). */
function writeFiles(root: string, files: Record<string, string>): void {
  for (const [path, contents] of Object.entries(files)) {
    const absolute = join(root, path);
    mkdirSync(dirname(absolute), { recursive: true });
    writeFileSync(absolute, contents);
  }
}

export interface BareRepo {
  /** The bare repository's absolute path. */
  path: string;
  /** The `file://` URL to hand `POST /projects` as `remote_url`. */
  url: string;
  /** The commit the initial branch points at. */
  initialCommit: string;
  /** The branch that commit is on; `main` unless overridden. */
  branch: string;
}

export interface CreateBareRepoOptions {
  /** Extra files in the initial commit, path → contents. */
  files?: Record<string, string>;
  /** The default branch; `main` unless given. */
  branch?: string;
}

/**
 * Creates `<reposDir>/<name>-<hex>.git` with one commit on `main` and returns
 * the `file://` URL a project is created from.
 *
 * The commit is pushed from a throw-away non-bare clone — a bare repository has
 * no work tree to commit in — and the bare `HEAD` is pointed at the branch
 * afterwards, because that symref is what the orchestrator's
 * `ls-remote --symref origin HEAD` reads the default branch off.
 */
export function createBareRepo(
  name: string,
  opts: CreateBareRepoOptions = {},
): BareRepo {
  const branch = opts.branch ?? "main";
  const root = reposDir();
  mkdirSync(root, { recursive: true });

  const path = join(root, `${name}-${randomSuffix()}.git`);
  git(root, ["init", "--bare", "-b", branch, path]);

  const work = mkdtempSync(join(tmpdir(), "mars-e2e-repo-"));
  try {
    git(work, ["init", "-b", branch, "."]);
    writeFiles(work, {
      "README.md": `# ${name}\n\nAn end-to-end fixture repository.\n`,
      ...(opts.files ?? {}),
    });
    git(work, ["add", "-A"]);
    git(work, [...COMMITTER, "commit", "-m", "Initial commit"]);
    git(work, ["push", path, `HEAD:refs/heads/${branch}`]);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }

  git(path, ["symbolic-ref", "HEAD", `refs/heads/${branch}`]);
  const initialCommit = git(path, ["rev-parse", branch]);

  return { path, url: `file://${path}`, initialCommit, branch };
}

/**
 * Adds a commit to the upstream's default branch and returns its id — the
 * upstream movement fetch, merge and conflict scenarios are built on.
 */
export function commitToBareRepo(
  repo: BareRepo,
  files: Record<string, string>,
  message: string,
): string {
  const work = mkdtempSync(join(tmpdir(), "mars-e2e-commit-"));
  try {
    git(work, ["clone", "--branch", repo.branch, repo.path, "."]);
    writeFiles(work, files);
    git(work, ["add", "-A"]);
    git(work, [...COMMITTER, "commit", "-m", message]);
    git(work, ["push", "origin", `HEAD:refs/heads/${repo.branch}`]);
    return git(work, ["rev-parse", "HEAD"]);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

/** `<DATA_DIR>/projects/<pid>/repo.git` (ARCHITECTURE.md, "Storage"). */
export function mirrorPath(projectId: string): string {
  return join(dataDir(), "projects", projectId, "repo.git");
}

/** `<DATA_DIR>/sessions/<sid>/work`, the session's clone (ARCHITECTURE.md, "Git model"). */
export function sessionWorkPath(sessionId: string): string {
  return join(dataDir(), "sessions", sessionId, "work");
}

/**
 * Commits work into a session's clone from the host, the way a scenario stands
 * in for an agent that wrote code. The clone is bind-mounted read-write into
 * the running container and the stub never touches the tree, so writing from
 * here is safe; files land owned by the host uid, which is uid 1000 inside the
 * container under keep-id.
 *
 * The orchestrator sets `user.name`/`user.email` on the clone; the `-c`
 * fallbacks below are only added when it has not, so a commit made here keeps
 * the session's own identity when there is one.
 */
export function commitInSessionWorkClone(
  sessionId: string,
  files: Record<string, string>,
  message: string,
): string {
  const work = sessionWorkPath(sessionId);
  writeFiles(work, files);
  git(work, ["add", "-A"]);

  const identity = hasIdentity(work) ? [] : COMMITTER;
  git(work, [...identity, "commit", "-m", message]);
  return git(work, ["rev-parse", "HEAD"]);
}

function hasIdentity(repoPath: string): boolean {
  for (const key of ["user.name", "user.email"]) {
    try {
      if (git(repoPath, ["config", "--get", key]) === "") return false;
    } catch {
      // `git config --get` exits 1 when the key is unset.
      return false;
    }
  }
  return true;
}

/** `git -C <repoPath> rev-parse <ref>`: the full object id. */
export function gitRevParse(repoPath: string, ref: string): string {
  return git(repoPath, ["rev-parse", ref]);
}

/** Whether `commit` is an ancestor of `ref` (a merge or a fast-forward landed). */
export function gitIsAncestor(
  repoPath: string,
  commit: string,
  ref: string,
): boolean {
  try {
    git(repoPath, ["merge-base", "--is-ancestor", commit, ref]);
    return true;
  } catch {
    // Exit code 1 is the answer "no"; anything else would have named the ref.
    return false;
  }
}
