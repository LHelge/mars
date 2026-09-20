import { describe, expect, it } from "vitest";

import { stripAnsi } from "./ansi";

describe("stripAnsi", () => {
  it("removes colour codes and keeps the text between them", () => {
    expect(stripAnsi("\u001B[31merror\u001B[0m: not found")).toBe(
      "error: not found",
    );
    expect(stripAnsi("\u001B[1;38;5;214mwarn\u001B[m")).toBe("warn");
  });

  it("removes cursor movement and mode switches", () => {
    expect(stripAnsi("a\u001B[2K\u001B[1Gb")).toBe("ab");
    expect(stripAnsi("\u001B[?25lhidden\u001B[?25h")).toBe("hidden");
    expect(stripAnsi("\u001B[HA\u001B[2JB")).toBe("AB");
  });

  it("removes OSC sequences whatever terminates them", () => {
    expect(stripAnsi("\u001B]0;a title\u0007done")).toBe("done");
    expect(stripAnsi("\u001B]8;;https://example.invalid\u001B\\link")).toBe(
      "link",
    );
  });

  it("normalises CRLF to LF and leaves lone carriage returns", () => {
    expect(stripAnsi("one\r\ntwo\r\n")).toBe("one\ntwo\n");
    expect(stripAnsi("50%\r100%")).toBe("50%\r100%");
  });

  it("leaves plain text untouched", () => {
    expect(stripAnsi("no escapes here")).toBe("no escapes here");
    expect(stripAnsi("")).toBe("");
  });
});
