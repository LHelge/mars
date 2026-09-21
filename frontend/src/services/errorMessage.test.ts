// The one error-to-text table (`SPEC.md`, "Frontend", Failure messages).

import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "./apiClient";
import {
  errorMessage,
  GENERIC_FAILURE,
  isNetworkFailure,
  isNotFound,
  logUnexpected,
  MessageError,
  statusMessage,
  UNREACHABLE,
} from "./errorMessage";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("errorMessage", () => {
  const table: [string, unknown, string][] = [
    [
      "a validation 400 in the server's words",
      new ApiError(400, "name must not be empty"),
      "name must not be empty",
    ],
    [
      "a 409 in the server's words",
      new ApiError(409, "session is running"),
      "session is running",
    ],
    [
      "a 5xx that carried words, in those words",
      new ApiError(503, "orchestrator restarting"),
      "orchestrator restarting",
    ],
    [
      "a proxy's wordless 502 as unreachable",
      new ApiError(502, ""),
      UNREACHABLE,
    ],
    ["a wordless 503 as unreachable", new ApiError(503, ""), UNREACHABLE],
    ["a wordless 504 as unreachable", new ApiError(504, ""), UNREACHABLE],
    ["a wordless 500 by its status", new ApiError(500, ""), "HTTP 500"],
    ["a wordless 404 by its status", new ApiError(404, "   "), "HTTP 404"],
    [
      "a network failure as unreachable",
      new TypeError("Failed to fetch"),
      UNREACHABLE,
    ],
    [
      "a client-decided message as itself",
      new MessageError("Not signed in"),
      "Not signed in",
    ],
    ["a bug here generically", new Error("boom"), GENERIC_FAILURE],
    ["a thrown string generically", "boom", GENERIC_FAILURE],
    ["undefined generically", undefined, GENERIC_FAILURE],
  ];

  it.each(table)("reads %s", (_name, caught, expected) => {
    expect(errorMessage(caught)).toBe(expected);
  });

  it("never returns an empty string", () => {
    for (const [, caught] of table) {
      expect(errorMessage(caught).length).toBeGreaterThan(0);
    }
    expect(errorMessage(new ApiError(0, "")).length).toBeGreaterThan(0);
  });

  it("uses the caller's fallback only where it has nothing better", () => {
    expect(errorMessage(new Error("boom"), "Could not load it.")).toBe(
      "Could not load it.",
    );
    expect(
      errorMessage(new ApiError(409, "session is running"), "Could not."),
    ).toBe("session is running");
    expect(errorMessage(new TypeError("Failed to fetch"), "Could not.")).toBe(
      UNREACHABLE,
    );
  });

  it("does not log: it is called from render", () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    errorMessage(new Error("boom"));
    errorMessage(new TypeError("Failed to fetch"));
    expect(logged).not.toHaveBeenCalled();
  });
});

describe("statusMessage", () => {
  it("names the statuses a proxy writes for an orchestrator that is not there", () => {
    expect(statusMessage(502)).toBe(UNREACHABLE);
    expect(statusMessage(503)).toBe(UNREACHABLE);
    expect(statusMessage(504)).toBe(UNREACHABLE);
  });

  it("names everything else by its number", () => {
    expect(statusMessage(500)).toBe("HTTP 500");
    expect(statusMessage(418)).toBe("HTTP 418");
  });
});

describe("isNotFound", () => {
  it("is the 404 and nothing else", () => {
    expect(isNotFound(new ApiError(404, "task not found"))).toBe(true);
    expect(isNotFound(new ApiError(403, "forbidden"))).toBe(false);
    expect(isNotFound(new TypeError("Failed to fetch"))).toBe(false);
  });
});

describe("isNetworkFailure", () => {
  it("is the `TypeError` `fetch` rejects with", () => {
    expect(isNetworkFailure(new TypeError("Failed to fetch"))).toBe(true);
    expect(isNetworkFailure(new Error("boom"))).toBe(false);
    expect(isNetworkFailure(new ApiError(502, ""))).toBe(false);
  });
});

describe("logUnexpected", () => {
  it("logs a bug here", () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const boom = new Error("boom");
    logUnexpected(boom);
    expect(logged).toHaveBeenCalledWith(boom);
  });

  it("stays quiet for an answer, a lost connection and our own wording", () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    logUnexpected(new ApiError(409, "session is running"));
    logUnexpected(new TypeError("Failed to fetch"));
    logUnexpected(new MessageError("Not signed in"));
    expect(logged).not.toHaveBeenCalled();
  });
});
