import { describe, expect, it } from "vitest";

import {
  DEFAULT_HISTORY_PAGE_SIZE,
  MAX_HISTORY_PAGE_SIZE,
  parseHistoryPageSize,
} from "./historyPageSize";

describe("parseHistoryPageSize", () => {
  it("defaults when the build sets nothing", () => {
    expect(parseHistoryPageSize(undefined)).toBe(DEFAULT_HISTORY_PAGE_SIZE);
    expect(parseHistoryPageSize("")).toBe(DEFAULT_HISTORY_PAGE_SIZE);
  });

  it("takes a whole number within the endpoint's cap", () => {
    expect(parseHistoryPageSize("100")).toBe(100);
    expect(parseHistoryPageSize(" 1 ")).toBe(1);
    expect(parseHistoryPageSize(String(MAX_HISTORY_PAGE_SIZE))).toBe(
      MAX_HISTORY_PAGE_SIZE,
    );
  });

  it("defaults rather than send a limit the endpoint refuses", () => {
    for (const raw of ["0", "501", "-5", "1.5", "many", "1e2"]) {
      expect(parseHistoryPageSize(raw)).toBe(DEFAULT_HISTORY_PAGE_SIZE);
    }
  });
});
