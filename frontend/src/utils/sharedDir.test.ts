import { describe, expect, it } from "vitest";
import {
  CONTAINER_PATH_MESSAGES,
  SHARED_DIR_NAME_MESSAGE,
  SHARED_DIR_PRESETS,
  validateContainerPath,
  validateSharedDir,
  validateSharedDirName,
} from "./sharedDir";

describe("validateSharedDirName", () => {
  it("accepts the documented pattern", () => {
    for (const raw of ["target", "n", "0", "node_modules", "a-b_c9", "9lives"]) {
      expect(validateSharedDirName(raw)).toBeNull();
    }
    expect(validateSharedDirName("t".repeat(64))).toBeNull();
  });

  it("rejects everything else with the one name message", () => {
    for (const raw of [
      "",
      "Target",
      "-target",
      "_target",
      ".target",
      "..",
      "tar get",
      "tar/get",
      "tär",
      "target!",
      "t".repeat(65),
    ]) {
      expect(validateSharedDirName(raw)).toBe(SHARED_DIR_NAME_MESSAGE);
    }
  });

  it("trims the surrounding whitespace the server trims", () => {
    expect(validateSharedDirName("  target ")).toBeNull();
  });
});

describe("validateContainerPath", () => {
  it("accepts normalised absolute paths outside the reserved set", () => {
    for (const raw of [
      "/session/work/target",
      "/session/workspace",
      "/session/work-tree",
      "/session/home/.cargo/registry",
      "/session/log/keep",
      "/session/mcp.json.bak",
      "/cache",
      "/datafiles",
    ]) {
      expect(validateContainerPath(raw)).toBeNull();
    }
  });

  it("names the one rule a path breaks", () => {
    const cases: [string, string][] = [
      ["", CONTAINER_PATH_MESSAGES.empty],
      ["   ", CONTAINER_PATH_MESSAGES.empty],
      [`/${"c".repeat(4096)}`, CONTAINER_PATH_MESSAGES.tooLong],
      ["target", CONTAINER_PATH_MESSAGES.absolute],
      ["C:\\cache", CONTAINER_PATH_MESSAGES.absolute],
      ["../target", CONTAINER_PATH_MESSAGES.absolute],
      ["/a/", CONTAINER_PATH_MESSAGES.trailingSlash],
      ["/a//b", CONTAINER_PATH_MESSAGES.repeatedSlash],
      ["/a/../b", CONTAINER_PATH_MESSAGES.dotSegment],
      ["/a/./b", CONTAINER_PATH_MESSAGES.dotSegment],
      ["/my cache", CONTAINER_PATH_MESSAGES.whitespace],
      ["/cache\u0000dir", CONTAINER_PATH_MESSAGES.whitespace],
      ["/data", CONTAINER_PATH_MESSAGES.data],
      ["/data/x", CONTAINER_PATH_MESSAGES.data],
      ["/", CONTAINER_PATH_MESSAGES.reserved],
      ["/session", CONTAINER_PATH_MESSAGES.reserved],
      ["/session/work", CONTAINER_PATH_MESSAGES.reserved],
      ["/session/home", CONTAINER_PATH_MESSAGES.reserved],
      ["/session/log", CONTAINER_PATH_MESSAGES.reserved],
      ["/session/mcp.json", CONTAINER_PATH_MESSAGES.reserved],
      ["/session/mcp.json/x", CONTAINER_PATH_MESSAGES.belowFile],
    ];
    for (const [raw, message] of cases) {
      expect(validateContainerPath(raw), raw).toBe(message);
    }
  });

  it("accepts the longest path the orchestrator accepts", () => {
    expect(validateContainerPath(`/${"c".repeat(4095)}`)).toBeNull();
  });
});

describe("validateSharedDir", () => {
  it("accepts every starting point README recommends", () => {
    for (const preset of SHARED_DIR_PRESETS) {
      expect(
        validateSharedDir(preset.name, preset.container_path),
        preset.name,
      ).toEqual({ name: null, containerPath: null });
    }
  });

  it("reports both fields at once", () => {
    expect(validateSharedDir("Target", "/data/x")).toEqual({
      name: SHARED_DIR_NAME_MESSAGE,
      containerPath: CONTAINER_PATH_MESSAGES.data,
    });
  });
});
