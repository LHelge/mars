import { describe, expect, it } from "vitest";
import {
  formatDateTime,
  formatRelative,
  formatTokens,
  formatUsd,
  formatUtc,
  PLACEHOLDER,
  shortId,
} from "./format";

const NOW = new Date("2026-03-01T12:00:00Z");

describe("formatRelative", () => {
  it("answers the placeholder for a missing timestamp", () => {
    expect(formatRelative(null, NOW)).toBe(PLACEHOLDER);
    expect(formatRelative(undefined, NOW)).toBe(PLACEHOLDER);
    expect(formatRelative("not a date", NOW)).toBe(PLACEHOLDER);
  });

  it("scales from seconds to days", () => {
    expect(formatRelative("2026-03-01T11:59:31Z", NOW)).toBe("just now");
    expect(formatRelative("2026-03-01T11:56:00Z", NOW)).toBe("4m ago");
    expect(formatRelative("2026-03-01T09:00:00Z", NOW)).toBe("3h ago");
    expect(formatRelative("2026-02-18T12:00:00Z", NOW)).toBe("11d ago");
  });

  it("clamps clock skew instead of counting backwards", () => {
    expect(formatRelative("2026-03-01T12:05:00Z", NOW)).toBe("just now");
  });
});

describe("formatDateTime", () => {
  it("answers the placeholder for a missing timestamp", () => {
    expect(formatDateTime(null)).toBe(PLACEHOLDER);
    expect(formatDateTime("")).toBe(PLACEHOLDER);
  });

  it("formats a real timestamp", () => {
    expect(formatDateTime("2026-03-01T12:00:00Z")).not.toBe(PLACEHOLDER);
  });
});

describe("formatUtc", () => {
  it("answers the placeholder for a missing timestamp", () => {
    expect(formatUtc(null)).toBe(PLACEHOLDER);
    expect(formatUtc("")).toBe(PLACEHOLDER);
    expect(formatUtc("not a date")).toBe(PLACEHOLDER);
  });

  it("says which zone it is in, and shows that zone's clock", () => {
    const formatted = formatUtc("2026-03-01T12:00:00Z");

    expect(formatted).toMatch(/ UTC$/);
    // Whatever the runner's locale, the hour is the UTC one and not the
    // machine's — the whole point of showing it beside the local time.
    expect(formatted).toContain("12:00");
  });
});

describe("formatUsd", () => {
  it("always shows two decimals", () => {
    expect(formatUsd(0)).toBe("$0.00");
    expect(formatUsd(1.2)).toBe("$1.20");
    expect(formatUsd(12.345)).toBe("$12.35");
  });

  it("answers the placeholder for a missing amount", () => {
    expect(formatUsd(null)).toBe(PLACEHOLDER);
    expect(formatUsd(Number.NaN)).toBe(PLACEHOLDER);
  });
});

describe("formatTokens", () => {
  it("groups thousands", () => {
    // The separator is the runner's locale; only the digits are ours.
    expect(formatTokens(1234567).replace(/\D/g, "")).toBe("1234567");
    expect(formatTokens(0)).toBe("0");
  });

  it("answers the placeholder for a missing counter", () => {
    expect(formatTokens(null)).toBe(PLACEHOLDER);
    expect(formatTokens(Number.NaN)).toBe(PLACEHOLDER);
  });
});

describe("shortId", () => {
  it("keeps the first twelve characters by default", () => {
    expect(shortId("0123456789abcdef0123")).toBe("0123456789ab");
  });

  it("leaves an id shorter than the cut alone", () => {
    expect(shortId("abc")).toBe("abc");
  });

  it("answers the placeholder for a missing id", () => {
    expect(shortId(null)).toBe(PLACEHOLDER);
    expect(shortId("")).toBe(PLACEHOLDER);
  });
});
