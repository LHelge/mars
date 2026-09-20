// The shared-directory name and container-path rules of `SPEC.md`, "Shared
// directories", checked in the browser so the add form answers while the user
// is still typing. The orchestrator applies the same rules and its 400 is the
// authority; this is a courtesy, and the form shows the server's message
// whenever one arrives.
//
// The rules are checked in the order the orchestrator checks them and each has
// a message of its own, so the field names the one thing to fix rather than
// restating the whole contract.

/** 1–64 characters, `[a-z0-9][a-z0-9_-]*`. */
export const SHARED_DIR_NAME_RE = /^[a-z0-9][a-z0-9_-]{0,63}$/;

/** The one message for every shape of a bad name, as the server phrases it. */
export const SHARED_DIR_NAME_MESSAGE =
  "Name must be 1–64 characters of lower-case letters, digits, underscores and hyphens, starting with a letter or digit";

/** Linux's `PATH_MAX`: a longer path could never be mounted. */
export const MAX_CONTAINER_PATH_BYTES = 4096;

/** One message per path rule; the form shows exactly one of them. */
export const CONTAINER_PATH_MESSAGES = {
  empty: "Container path is required",
  tooLong: `Container path must be at most ${String(MAX_CONTAINER_PATH_BYTES)} bytes`,
  absolute: "Container path must be an absolute path",
  trailingSlash: "Container path must not end in a slash",
  repeatedSlash: "Container path must not contain repeated slashes",
  dotSegment: "Container path must not contain . or .. segments",
  whitespace: "Container path must not contain whitespace or control characters",
  data: "Container path must not be /data or below it",
  reserved:
    "Container path must not be, or contain, /session/work, /session/home, /session/log or /session/mcp.json",
  belowFile: "Container path must not be below /session/mcp.json, which is a file",
} as const;

/** The session's own machinery: a mount may sit below these, never over them. */
const RESERVED_PATHS: readonly (readonly string[])[] = [
  ["session", "work"],
  ["session", "home"],
  ["session", "log"],
  ["session", "mcp.json"],
];

/** The one reserved path that is a file, so nothing can be mounted below it. */
const MCP_CONFIG_PATH: readonly string[] = ["session", "mcp.json"];

/** The orchestrator's own volume. */
const DATA_SEGMENT = "data";

/** Whitespace, or one of the C0, DEL and C1 control characters. */
const WHITESPACE = /\s/u;

function hasUnprintable(raw: string): boolean {
  for (const character of raw) {
    const code = character.codePointAt(0) ?? 0;
    const control = code <= 0x1f || (code >= 0x7f && code <= 0x9f);
    if (control || WHITESPACE.test(character)) {
      return true;
    }
  }
  return false;
}

/** The starting points of `README.md`, "Operating notes", as form presets. */
export interface SharedDirPreset {
  ecosystem: string;
  name: string;
  container_path: string;
}

export const SHARED_DIR_PRESETS: readonly SharedDirPreset[] = [
  { ecosystem: "Rust", name: "target", container_path: "/session/work/target" },
  {
    ecosystem: "Rust",
    name: "cargo-registry",
    container_path: "/session/home/.cargo/registry",
  },
  { ecosystem: "Node", name: "npm-cache", container_path: "/session/home/.npm" },
  { ecosystem: "Go", name: "go-mod", container_path: "/session/home/go/pkg/mod" },
  {
    ecosystem: "Go",
    name: "go-build",
    container_path: "/session/home/.cache/go-build",
  },
  {
    ecosystem: "Python",
    name: "uv-cache",
    container_path: "/session/home/.cache/uv",
  },
  { ecosystem: "JVM", name: "m2", container_path: "/session/home/.m2" },
  { ecosystem: "JVM", name: "gradle", container_path: "/session/home/.gradle" },
];

/** The message for a name that cannot be stored, or `null` when it can. */
export function validateSharedDirName(name: string): string | null {
  return SHARED_DIR_NAME_RE.test(name.trim()) ? null : SHARED_DIR_NAME_MESSAGE;
}

/**
 * The message for a path that cannot be mounted, or `null` when it can.
 *
 * A path that needs normalising is refused rather than rewritten, so what the
 * form shows is exactly what the engine is given.
 */
export function validateContainerPath(containerPath: string): string | null {
  const raw = containerPath.trim();

  if (raw === "") {
    return CONTAINER_PATH_MESSAGES.empty;
  }
  if (new TextEncoder().encode(raw).length > MAX_CONTAINER_PATH_BYTES) {
    return CONTAINER_PATH_MESSAGES.tooLong;
  }

  const segments = splitSegments(raw);
  if (typeof segments === "string") {
    return segments;
  }

  if (hasUnprintable(raw)) {
    return CONTAINER_PATH_MESSAGES.whitespace;
  }

  if (segments[0] === DATA_SEGMENT) {
    return CONTAINER_PATH_MESSAGES.data;
  }

  // The root — an empty segment list — is an ancestor of all four and is
  // caught here rather than needing a case of its own.
  for (const reserved of RESERVED_PATHS) {
    if (isAncestorOrEqual(segments, reserved)) {
      return CONTAINER_PATH_MESSAGES.reserved;
    }
  }

  if (
    segments.length > MCP_CONFIG_PATH.length &&
    MCP_CONFIG_PATH.every((segment, index) => segments[index] === segment)
  ) {
    return CONTAINER_PATH_MESSAGES.belowFile;
  }

  return null;
}

/** Per-field messages, `null` where the field is acceptable. */
export interface SharedDirErrors {
  name: string | null;
  containerPath: string | null;
}

export function validateSharedDir(
  name: string,
  containerPath: string,
): SharedDirErrors {
  return {
    name: validateSharedDirName(name),
    containerPath: validateContainerPath(containerPath),
  };
}

/**
 * The segments of an absolute, normalised path, or the message for the rule it
 * broke. `/` yields an empty list, which is why the root is not treated as a
 * trailing slash.
 */
function splitSegments(raw: string): string[] | string {
  if (!raw.startsWith("/")) {
    return CONTAINER_PATH_MESSAGES.absolute;
  }
  const rest = raw.slice(1);
  if (rest === "") {
    return [];
  }
  if (rest.endsWith("/")) {
    return CONTAINER_PATH_MESSAGES.trailingSlash;
  }

  const segments = rest.split("/");
  for (const segment of segments) {
    if (segment === "") {
      return CONTAINER_PATH_MESSAGES.repeatedSlash;
    }
    if (segment === "." || segment === "..") {
      return CONTAINER_PATH_MESSAGES.dotSegment;
    }
  }
  return segments;
}

/**
 * Is `candidate` the same path as `reserved`, or an ancestor of it? Compared
 * segment by segment, never as strings: `/session/work-tree` shares a string
 * prefix with `/session/work` and is a perfectly good mount point, while
 * `/session` shares none and is the ancestor that would swallow it.
 */
function isAncestorOrEqual(
  candidate: readonly string[],
  reserved: readonly string[],
): boolean {
  return (
    candidate.length <= reserved.length &&
    candidate.every((segment, index) => reserved[index] === segment)
  );
}
