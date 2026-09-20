import { describe, expect, it } from "vitest";

import { LABEL_RULE, labelsError, parseLabels } from "./taskLabels";

describe("parseLabels", () => {
  it("splits on commas and whitespace alike", () => {
    expect(parseLabels("backend, migration  db").labels).toEqual([
      "backend",
      "migration",
      "db",
    ]);
  });

  it("is empty for an empty field", () => {
    expect(parseLabels("   ")).toEqual({ labels: [], invalid: [] });
  });

  it("drops a repeat rather than calling it an error", () => {
    const parsed = parseLabels("api, api");
    expect(parsed.labels).toEqual(["api"]);
    expect(parsed.invalid).toEqual([]);
  });

  it("collects what the API would refuse", () => {
    const parsed = parseLabels("ok Backend -leading api!");
    expect(parsed.labels).toEqual(["ok"]);
    expect(parsed.invalid).toEqual(["Backend", "-leading", "api!"]);
  });

  it("accepts 32 characters and refuses 33", () => {
    expect(parseLabels("a".repeat(32)).invalid).toEqual([]);
    expect(parseLabels("a".repeat(33)).labels).toEqual([]);
  });
});

describe("labelsError", () => {
  it("is null when every entry is spelled right", () => {
    expect(labelsError("backend db_2 x-y")).toBeNull();
  });

  it("names the entries that were refused", () => {
    const message = labelsError("ok NOPE");
    expect(message).toContain(LABEL_RULE);
    expect(message).toContain("NOPE");
  });
});
